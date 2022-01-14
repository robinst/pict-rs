#[cfg(feature = "io-uring")]
pub(crate) use io_uring::File;

#[cfg(not(feature = "io-uring"))]
pub(crate) use tokio_file::File;

#[cfg(not(feature = "io-uring"))]
mod tokio_file {
    use crate::{store::file_store::FileError, Either};
    use actix_web::web::{Bytes, BytesMut};
    use futures_util::stream::{Stream, StreamExt};
    use std::{io::SeekFrom, path::Path};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
    use tokio_util::codec::{BytesCodec, FramedRead};

    pub(crate) struct File {
        inner: tokio::fs::File,
    }

    impl File {
        pub(crate) async fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
            Ok(File {
                inner: tokio::fs::File::open(path).await?,
            })
        }

        pub(crate) async fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
            Ok(File {
                inner: tokio::fs::File::create(path).await?,
            })
        }

        pub(crate) async fn write_from_bytes(&mut self, mut bytes: Bytes) -> std::io::Result<()> {
            self.inner.write_all_buf(&mut bytes).await?;
            Ok(())
        }

        pub(crate) async fn write_from_stream<S>(&mut self, stream: S) -> std::io::Result<()>
        where
            S: Stream<Item = std::io::Result<Bytes>>,
        {
            futures_util::pin_mut!(stream);

            while let Some(res) = stream.next().await {
                let mut bytes = res?;

                self.inner.write_all_buf(&mut bytes).await?;
            }

            Ok(())
        }

        pub(crate) async fn write_from_async_read<R>(
            &mut self,
            mut reader: R,
        ) -> std::io::Result<()>
        where
            R: AsyncRead + Unpin,
        {
            tokio::io::copy(&mut reader, &mut self.inner).await?;
            Ok(())
        }

        pub(crate) async fn close(self) -> std::io::Result<()> {
            Ok(())
        }

        pub(crate) async fn read_to_async_write<W>(&mut self, writer: &mut W) -> std::io::Result<()>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            tokio::io::copy(&mut self.inner, writer).await?;
            Ok(())
        }

        pub(crate) async fn read_to_stream(
            mut self,
            from_start: Option<u64>,
            len: Option<u64>,
        ) -> Result<impl Stream<Item = std::io::Result<Bytes>>, FileError> {
            let obj = match (from_start, len) {
                (Some(lower), Some(upper)) => {
                    self.inner.seek(SeekFrom::Start(lower)).await?;
                    Either::left(self.inner.take(upper))
                }
                (None, Some(upper)) => Either::left(self.inner.take(upper)),
                (Some(lower), None) => {
                    self.inner.seek(SeekFrom::Start(lower)).await?;
                    Either::right(self.inner)
                }
                (None, None) => Either::right(self.inner),
            };

            Ok(BytesFreezer::new(FramedRead::new(obj, BytesCodec::new())))
        }
    }

    pin_project_lite::pin_project! {
        struct BytesFreezer<S> {
            #[pin]
            inner: S,
        }
    }

    impl<S> BytesFreezer<S> {
        fn new(inner: S) -> Self {
            BytesFreezer { inner }
        }
    }

    impl<S, E> Stream for BytesFreezer<S>
    where
        S: Stream<Item = Result<BytesMut, E>> + Unpin,
    {
        type Item = Result<Bytes, E>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            let this = self.as_mut().project();

            this.inner
                .poll_next(cx)
                .map(|opt| opt.map(|res| res.map(BytesMut::freeze)))
        }
    }
}

#[cfg(feature = "io-uring")]
mod io_uring {
    use crate::store::file_store::FileError;
    use actix_web::web::{Bytes, BytesMut};
    use futures_util::stream::{Stream, StreamExt};
    use std::{
        convert::TryInto,
        fs::Metadata,
        future::Future,
        path::{Path, PathBuf},
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio_uring::{
        buf::{IoBuf, IoBufMut},
        BufResult,
    };

    pub(crate) struct File {
        path: PathBuf,
        inner: tokio_uring::fs::File,
    }

    impl File {
        pub(crate) async fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
            tracing::info!("Opening io-uring file: {:?}", path.as_ref());
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: tokio_uring::fs::File::open(path).await?,
            })
        }

        pub(crate) async fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
            tracing::info!("Creating io-uring file: {:?}", path.as_ref());
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: tokio_uring::fs::File::create(path).await?,
            })
        }

        async fn metadata(&self) -> std::io::Result<Metadata> {
            tokio::fs::metadata(&self.path).await
        }

        pub(crate) async fn write_from_bytes(&mut self, mut buf: Bytes) -> std::io::Result<()> {
            let len: u64 = buf.len().try_into().unwrap();

            let mut cursor: u64 = 0;

            loop {
                if cursor == len {
                    break;
                }

                let cursor_usize: usize = cursor.try_into().unwrap();
                let (res, slice) = self.inner.write_at(buf.slice(cursor_usize..), cursor).await;
                let n: usize = res?;

                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }

                buf = slice.into_inner();
                let n: u64 = n.try_into().unwrap();
                cursor += n;
            }

            self.inner.sync_all().await?;

            Ok(())
        }

        pub(crate) async fn write_from_stream<S>(&mut self, stream: S) -> std::io::Result<()>
        where
            S: Stream<Item = std::io::Result<Bytes>>,
        {
            futures_util::pin_mut!(stream);
            let mut cursor: u64 = 0;

            while let Some(res) = stream.next().await {
                let mut buf = res?;

                let len = buf.len();
                let mut position = 0;

                loop {
                    if position == len {
                        break;
                    }

                    let position_u64: u64 = position.try_into().unwrap();
                    let (res, slice) = self
                        .write_at(buf.slice(position..len), cursor + position_u64)
                        .await;

                    let n = res?;
                    if n == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }

                    position += n;

                    buf = slice.into_inner();
                }

                let position: u64 = position.try_into().unwrap();
                cursor += position;
            }

            self.inner.sync_all().await?;

            Ok(())
        }

        pub(crate) async fn write_from_async_read<R>(
            &mut self,
            mut reader: R,
        ) -> std::io::Result<()>
        where
            R: AsyncRead + Unpin,
        {
            let mut cursor: u64 = 0;

            loop {
                let max_size = 65_536;
                let mut buf = Vec::with_capacity(max_size.try_into().unwrap());

                let n = (&mut reader).take(max_size).read_to_end(&mut buf).await?;

                if n == 0 {
                    break;
                }

                let mut position = 0;

                loop {
                    if position == n {
                        break;
                    }

                    let position_u64: u64 = position.try_into().unwrap();
                    let (res, slice) = self
                        .write_at(buf.slice(position..n), cursor + position_u64)
                        .await;

                    let n = res?;
                    if n == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }

                    position += n;

                    buf = slice.into_inner();
                }

                let position: u64 = position.try_into().unwrap();
                cursor += position;
            }

            self.inner.sync_all().await?;

            Ok(())
        }

        pub(crate) async fn close(self) -> std::io::Result<()> {
            self.inner.close().await
        }

        pub(crate) async fn read_to_async_write<W>(&mut self, writer: &mut W) -> std::io::Result<()>
        where
            W: AsyncWrite + Unpin + ?Sized,
        {
            let metadata = self.metadata().await?;
            let size = metadata.len();

            let mut cursor: u64 = 0;

            loop {
                if cursor == size {
                    break;
                }

                let max_size = (size - cursor).min(65_536);
                let buf = BytesMut::with_capacity(max_size.try_into().unwrap());

                let (res, buf): (_, BytesMut) = self.read_at(buf, cursor).await;
                let n: usize = res?;

                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }

                writer.write_all(&buf[0..n]).await?;

                let n: u64 = n.try_into().unwrap();
                cursor += n;
            }

            Ok(())
        }

        pub(crate) async fn read_to_stream(
            self,
            from_start: Option<u64>,
            len: Option<u64>,
        ) -> Result<impl Stream<Item = std::io::Result<Bytes>>, FileError> {
            let size = self.metadata().await?.len();

            let cursor = from_start.unwrap_or(0);
            let size = len.unwrap_or(size - cursor) + cursor;

            Ok(BytesStream {
                state: ReadFileState::File {
                    file: Some(self),
                    bytes: Some(BytesMut::new()),
                },
                size,
                cursor,
                callback: read_file,
            })
        }

        async fn read_at<T: IoBufMut>(&self, buf: T, pos: u64) -> BufResult<usize, T> {
            self.inner.read_at(buf, pos).await
        }

        async fn write_at<T: IoBuf>(&self, buf: T, pos: u64) -> BufResult<usize, T> {
            self.inner.write_at(buf, pos).await
        }
    }

    pin_project_lite::pin_project! {
        struct BytesStream<F, Fut> {
            #[pin]
            state: ReadFileState<Fut>,
            size: u64,
            cursor: u64,
            #[pin]
            callback: F,
        }
    }

    pin_project_lite::pin_project! {
        #[project = ReadFileStateProj]
        #[project_replace = ReadFileStateProjReplace]
        enum ReadFileState<Fut> {
            File {
                file: Option<File>,
                bytes: Option<BytesMut>,
            },
            Future {
                #[pin]
                fut: Fut,
            },
        }
    }

    async fn read_file(
        file: File,
        buf: BytesMut,
        cursor: u64,
    ) -> (File, BufResult<usize, BytesMut>) {
        let buf_res = file.read_at(buf, cursor).await;

        (file, buf_res)
    }

    impl<F, Fut> Stream for BytesStream<F, Fut>
    where
        F: Fn(File, BytesMut, u64) -> Fut,
        Fut: Future<Output = (File, BufResult<usize, BytesMut>)> + 'static,
    {
        type Item = std::io::Result<Bytes>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut this = self.as_mut().project();

            match this.state.as_mut().project() {
                ReadFileStateProj::File { file, bytes } => {
                    let cursor = *this.cursor;
                    let max_size = *this.size - *this.cursor;

                    if max_size == 0 {
                        return Poll::Ready(None);
                    }

                    let capacity = max_size.min(65_356) as usize;
                    let mut bytes = bytes.take().unwrap();
                    let file = file.take().unwrap();

                    if bytes.capacity() < capacity {
                        bytes.reserve(capacity - bytes.capacity());
                    }

                    let fut = (this.callback)(file, bytes, cursor);

                    this.state.project_replace(ReadFileState::Future { fut });
                    self.poll_next(cx)
                }
                ReadFileStateProj::Future { fut } => match fut.poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready((file, (Ok(n), mut buf))) => {
                        let bytes = buf.split_off(n);

                        this.state.project_replace(ReadFileState::File {
                            file: Some(file),
                            bytes: Some(bytes),
                        });

                        let n: u64 = match n.try_into() {
                            Ok(n) => n,
                            Err(_) => {
                                return Poll::Ready(Some(Err(std::io::ErrorKind::Other.into())))
                            }
                        };
                        *this.cursor += n;

                        Poll::Ready(Some(Ok(buf.into())))
                    }
                    Poll::Ready((_, (Err(e), _))) => Poll::Ready(Some(Err(e))),
                },
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::io::Read;

        macro_rules! test_on_arbiter {
            ($fut:expr) => {
                actix_rt::System::new().block_on(async move {
                    let arbiter = actix_rt::Arbiter::new();

                    let (tx, rx) = tokio::sync::oneshot::channel();

                    arbiter.spawn(async move {
                        let handle = actix_rt::spawn($fut);

                        let _ = tx.send(handle.await.unwrap());
                    });

                    rx.await.unwrap()
                })
            };
        }

        const EARTH_GIF: &'static str = "client-examples/earth.gif";

        #[test]
        fn read() {
            let tmp = "/tmp/read-test";

            test_on_arbiter!(async move {
                let mut file = super::File::open(EARTH_GIF).await.unwrap();
                let mut tmp_file = tokio::fs::File::create(tmp).await.unwrap();
                file.read_to_async_write(&mut tmp_file).await.unwrap();
            });

            let mut source = std::fs::File::open(EARTH_GIF).unwrap();
            let mut dest = std::fs::File::open(tmp).unwrap();

            let mut source_bytes = Vec::new();
            source.read_to_end(&mut source_bytes).unwrap();

            let mut dest_bytes = Vec::new();
            dest.read_to_end(&mut dest_bytes).unwrap();

            drop(dest);
            std::fs::remove_file(tmp).unwrap();

            assert_eq!(source_bytes.len(), dest_bytes.len());
            assert_eq!(source_bytes, dest_bytes);
        }

        #[test]
        fn write() {
            let tmp = "/tmp/write-test";

            test_on_arbiter!(async move {
                let mut file = tokio::fs::File::open(EARTH_GIF).await.unwrap();
                let mut tmp_file = super::File::create(tmp).await.unwrap();
                tmp_file.write_from_async_read(&mut file).await.unwrap();
            });

            let mut source = std::fs::File::open(EARTH_GIF).unwrap();
            let mut dest = std::fs::File::open(tmp).unwrap();

            let mut source_bytes = Vec::new();
            source.read_to_end(&mut source_bytes).unwrap();

            let mut dest_bytes = Vec::new();
            dest.read_to_end(&mut dest_bytes).unwrap();

            drop(dest);
            std::fs::remove_file(tmp).unwrap();

            assert_eq!(source_bytes.len(), dest_bytes.len());
            assert_eq!(source_bytes, dest_bytes);
        }
    }
}
