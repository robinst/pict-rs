#[cfg(feature = "io-uring")]
pub(crate) use io_uring::File;

#[cfg(not(feature = "io-uring"))]
pub(crate) use tokio::fs::File;

#[cfg(feature = "io-uring")]
mod io_uring {
    use std::{
        convert::TryInto,
        fs::Metadata,
        future::Future,
        io::SeekFrom,
        path::{Path, PathBuf},
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};

    type IoFuture =
        Pin<Box<dyn Future<Output = (tokio_uring::fs::File, std::io::Result<usize>, Vec<u8>)>>>;

    type FlushFuture = Pin<Box<dyn Future<Output = (tokio_uring::fs::File, std::io::Result<()>)>>>;

    type ShutdownFuture = Pin<Box<dyn Future<Output = std::io::Result<()>>>>;

    type SeekFuture = Pin<Box<dyn Future<Output = std::io::Result<u64>>>>;

    enum FileState {
        Reading { future: IoFuture },
        Writing { future: IoFuture },
        Syncing { future: FlushFuture },
        Seeking { future: SeekFuture },
        Shutdown { future: ShutdownFuture },
        Pending,
    }

    impl FileState {
        fn take(&mut self) -> Self {
            std::mem::replace(self, FileState::Pending)
        }
    }

    pub(crate) struct File {
        path: PathBuf,
        inner: Option<tokio_uring::fs::File>,
        cursor: usize,
        wakers: Vec<Waker>,
        state: FileState,
    }

    impl File {
        pub(crate) async fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
            tracing::info!("Opening io-uring file");
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: Some(tokio_uring::fs::File::open(path).await?),
                cursor: 0,
                wakers: vec![],
                state: FileState::Pending,
            })
        }

        pub(crate) async fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
            tracing::info!("Creating io-uring file");
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: Some(tokio_uring::fs::File::create(path).await?),
                cursor: 0,
                wakers: vec![],
                state: FileState::Pending,
            })
        }

        pub(crate) async fn metadata(&self) -> std::io::Result<Metadata> {
            tokio::fs::metadata(&self.path).await
        }

        fn poll_read(
            &mut self,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
            mut future: IoFuture,
        ) -> Poll<std::io::Result<()>> {
            match Pin::new(&mut future).poll(cx) {
                Poll::Ready((file, Ok(bytes_read), vec)) => {
                    self.cursor += bytes_read;
                    self.inner = Some(file);
                    buf.put_slice(&vec[0..bytes_read]);

                    // Wake tasks waiting on read to complete
                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    Poll::Ready(Ok(()))
                }
                Poll::Ready((file, Err(err), _vec)) => {
                    self.inner = Some(file);
                    // Wake tasks waiting on read to complete
                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    Poll::Ready(Err(err))
                }
                Poll::Pending => {
                    self.state = FileState::Reading { future };

                    Poll::Pending
                }
            }
        }

        fn poll_write(
            &mut self,
            cx: &mut Context<'_>,
            mut future: IoFuture,
        ) -> Poll<std::io::Result<usize>> {
            match Pin::new(&mut future).poll(cx) {
                Poll::Ready((file, Ok(bytes_written), _vec)) => {
                    self.cursor += bytes_written;
                    self.inner = Some(file);

                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    Poll::Ready(Ok(bytes_written))
                }
                Poll::Ready((file, Err(err), _vec)) => {
                    self.inner = Some(file);

                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    Poll::Ready(Err(err))
                }
                Poll::Pending => {
                    self.state = FileState::Writing { future };

                    Poll::Pending
                }
            }
        }

        fn poll_flush(
            &mut self,
            cx: &mut Context<'_>,
            mut future: FlushFuture,
        ) -> Poll<std::io::Result<()>> {
            match Pin::new(&mut future).poll(cx) {
                Poll::Ready((file, res)) => {
                    self.inner = Some(file);

                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    Poll::Ready(res)
                }
                Poll::Pending => {
                    self.state = FileState::Syncing { future };

                    Poll::Pending
                }
            }
        }

        fn poll_shutdown(
            &mut self,
            cx: &mut Context<'_>,
            mut future: ShutdownFuture,
        ) -> Poll<std::io::Result<()>> {
            match Pin::new(&mut future).poll(cx) {
                Poll::Ready(res) => {
                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    Poll::Ready(res)
                }
                Poll::Pending => {
                    self.state = FileState::Shutdown { future };

                    Poll::Pending
                }
            }
        }

        fn poll_seek(
            &mut self,
            cx: &mut Context<'_>,
            mut future: SeekFuture,
        ) -> Poll<std::io::Result<u64>> {
            match Pin::new(&mut future).poll(cx) {
                Poll::Ready(Ok(new_position)) => {
                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    if let Ok(position) = new_position.try_into() {
                        self.cursor = position;
                        Poll::Ready(Ok(new_position))
                    } else {
                        Poll::Ready(Err(std::io::ErrorKind::Other.into()))
                    }
                }
                Poll::Ready(Err(err)) => {
                    for waker in self.wakers.drain(..) {
                        waker.wake();
                    }

                    Poll::Ready(Err(err))
                }
                Poll::Pending => {
                    self.state = FileState::Seeking { future };

                    Poll::Pending
                }
            }
        }

        fn prepare_read(&mut self, buf: &mut ReadBuf<'_>) -> IoFuture {
            let bytes_to_read = buf.remaining().min(65_536);

            let vec = vec![0u8; bytes_to_read];

            let file = self.inner.take().unwrap();
            let position: u64 = self.cursor.try_into().unwrap();

            Box::pin(async move {
                let (res, vec) = file.read_at(vec, position).await;
                (file, res, vec)
            })
        }

        fn prepare_write(&mut self, buf: &[u8]) -> IoFuture {
            let vec = buf.to_vec();

            let file = self.inner.take().unwrap();
            let position: u64 = self.cursor.try_into().unwrap();

            Box::pin(async move {
                let (res, vec) = file.write_at(vec, position).await;
                (file, res, vec)
            })
        }

        fn prepare_flush(&mut self) -> FlushFuture {
            let file = self.inner.take().unwrap();

            Box::pin(async move {
                let res = file.sync_all().await;
                (file, res)
            })
        }

        fn prepare_shutdown(&mut self) -> ShutdownFuture {
            let file = self.inner.take().unwrap();

            Box::pin(async move {
                file.sync_all().await?;
                file.close().await
            })
        }

        fn prepare_seek(&self, from_end: i64) -> SeekFuture {
            let path = self.path.clone();

            Box::pin(async move {
                let meta = tokio::fs::metadata(path).await?;
                let end = meta.len();

                if from_end < 0 {
                    let from_end = (-1) * from_end;
                    let from_end: u64 =
                        from_end.try_into().map_err(|_| std::io::ErrorKind::Other)?;

                    return Ok(end + from_end);
                }

                let from_end: u64 = from_end.try_into().map_err(|_| std::io::ErrorKind::Other)?;

                if from_end > end {
                    return Err(std::io::ErrorKind::Other.into());
                }

                Ok(end - from_end)
            })
        }

        fn register_waker<T>(&mut self, cx: &mut Context<'_>) -> Poll<T> {
            let already_registered = self.wakers.iter().any(|waker| cx.waker().will_wake(waker));

            if !already_registered {
                self.wakers.push(cx.waker().clone());
            }

            Poll::Pending
        }
    }

    impl AsyncRead for File {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            match self.state.take() {
                FileState::Pending => {
                    let future = (*self).prepare_read(buf);

                    (*self).poll_read(cx, buf, future)
                }
                FileState::Reading { future } => (*self).poll_read(cx, buf, future),
                _ => (*self).register_waker(cx),
            }
        }
    }

    impl AsyncWrite for File {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            match self.state.take() {
                FileState::Pending => {
                    let future = (*self).prepare_write(buf);

                    (*self).poll_write(cx, future)
                }
                FileState::Writing { future } => (*self).poll_write(cx, future),
                _ => (*self).register_waker(cx),
            }
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            match self.state.take() {
                FileState::Pending => {
                    let future = (*self).prepare_flush();

                    (*self).poll_flush(cx, future)
                }
                FileState::Syncing { future } => (*self).poll_flush(cx, future),
                _ => (*self).register_waker(cx),
            }
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            match self.state.take() {
                FileState::Pending => {
                    let future = (*self).prepare_shutdown();

                    (*self).poll_shutdown(cx, future)
                }
                FileState::Shutdown { future } => (*self).poll_shutdown(cx, future),
                _ => (*self).register_waker(cx),
            }
        }
    }

    impl AsyncSeek for File {
        fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
            match position {
                SeekFrom::Start(from_start) => {
                    self.cursor = from_start.try_into().unwrap();
                    Ok(())
                }
                SeekFrom::End(from_end) => match self.state.take() {
                    FileState::Pending => {
                        let future = self.prepare_seek(from_end);

                        self.state = FileState::Seeking { future };
                        Ok(())
                    }
                    _ => Err(std::io::ErrorKind::Other.into()),
                },
                SeekFrom::Current(from_current) => {
                    if from_current < 0 {
                        let to_subtract = (-1) * from_current;
                        let to_subtract: usize = to_subtract
                            .try_into()
                            .map_err(|_| std::io::ErrorKind::Other)?;

                        if to_subtract > self.cursor {
                            return Err(std::io::ErrorKind::Other.into());
                        }

                        self.cursor -= to_subtract;
                    } else {
                        let from_current: usize = from_current
                            .try_into()
                            .map_err(|_| std::io::ErrorKind::Other)?;

                        self.cursor += from_current;
                    }

                    Ok(())
                }
            }
        }

        fn poll_complete(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            match self.state.take() {
                FileState::Pending => Poll::Ready(Ok(self
                    .cursor
                    .try_into()
                    .map_err(|_| std::io::ErrorKind::Other)?)),
                FileState::Seeking { future } => (*self).poll_seek(cx, future),
                _ => Poll::Ready(Err(std::io::ErrorKind::Other.into())),
            }
        }
    }
}
