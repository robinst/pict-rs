use futures_util::stream::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[cfg(feature = "io-uring")]
pub(crate) use io_uring::File;

#[cfg(not(feature = "io-uring"))]
pub(crate) use tokio_file::File;

pin_project_lite::pin_project! {
    pub(super) struct CrateError<S> {
        #[pin]
        inner: S
    }
}

impl<S> CrateError<S> {
    pub(super) fn new(inner: S) -> Self {
        CrateError { inner }
    }
}

impl<T, E, S> Stream for CrateError<S>
where
    S: Stream<Item = Result<T, E>>,
    crate::error::Error: From<E>,
{
    type Item = Result<T, crate::error::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().project();

        this.inner
            .poll_next(cx)
            .map(|opt| opt.map(|res| res.map_err(Into::into)))
    }
}

#[cfg(not(feature = "io-uring"))]
mod tokio_file {
    use crate::Either;
    use actix_web::web::{Bytes, BytesMut};
    use futures_util::stream::Stream;
    use std::{fs::Metadata, io::SeekFrom, path::Path};
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

        pub(crate) async fn metadata(&self) -> std::io::Result<Metadata> {
            self.inner.metadata().await
        }

        pub(crate) async fn write_from_bytes<'a>(
            &'a mut self,
            mut bytes: Bytes,
        ) -> std::io::Result<()> {
            self.inner.write_all_buf(&mut bytes).await?;
            Ok(())
        }

        pub(crate) async fn write_from_async_read<'a, R>(
            &'a mut self,
            mut reader: R,
        ) -> std::io::Result<()>
        where
            R: AsyncRead + Unpin,
        {
            tokio::io::copy(&mut reader, &mut self.inner).await?;
            Ok(())
        }

        pub(crate) async fn read_to_async_write<'a, W>(
            &'a mut self,
            writer: &'a mut W,
        ) -> std::io::Result<()>
        where
            W: AsyncWrite + Unpin,
        {
            tokio::io::copy(&mut self.inner, writer).await?;
            Ok(())
        }

        pub(crate) async fn read_to_stream(
            mut self,
            from_start: Option<u64>,
            len: Option<u64>,
        ) -> Result<impl Stream<Item = Result<Bytes, crate::error::Error>>, crate::error::Error>
        {
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

            Ok(super::CrateError::new(BytesFreezer::new(FramedRead::new(
                obj,
                BytesCodec::new(),
            ))))
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
    use actix_web::web::Bytes;
    use futures_util::stream::Stream;
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
            tracing::info!("Opening io-uring file");
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: tokio_uring::fs::File::open(path).await?,
            })
        }

        pub(crate) async fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
            tracing::info!("Creating io-uring file");
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: tokio_uring::fs::File::create(path).await?,
            })
        }

        pub(crate) async fn metadata(&self) -> std::io::Result<Metadata> {
            tokio::fs::metadata(&self.path).await
        }

        pub(crate) async fn write_from_bytes<'a>(
            &'a mut self,
            bytes: Bytes,
        ) -> std::io::Result<()> {
            let mut buf = bytes.to_vec();
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

        pub(crate) async fn write_from_async_read<'a, R>(
            &'a mut self,
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

        pub(crate) async fn read_to_async_write<'a, W>(
            &'a mut self,
            writer: &mut W,
        ) -> std::io::Result<()>
        where
            W: AsyncWrite + Unpin,
        {
            let metadata = self.metadata().await?;
            let size = metadata.len();

            let mut cursor: u64 = 0;

            loop {
                if cursor == size {
                    break;
                }

                let max_size = (size - cursor).min(65_536);
                let buf = Vec::with_capacity(max_size.try_into().unwrap());

                let (res, mut buf): (_, Vec<u8>) = self.read_at(buf, cursor).await;
                let n: usize = res?;

                if n == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }

                writer.write_all(&mut buf[0..n]).await?;

                let n: u64 = n.try_into().unwrap();
                cursor += n;
            }

            Ok(())
        }

        pub(crate) async fn read_to_stream(
            self,
            from_start: Option<u64>,
            len: Option<u64>,
        ) -> Result<impl Stream<Item = Result<Bytes, crate::error::Error>>, crate::error::Error>
        {
            let size = self.metadata().await?.len();

            let cursor = from_start.unwrap_or(0);
            let size = len.unwrap_or(size - cursor) + cursor;

            Ok(super::CrateError {
                inner: BytesStream {
                    state: ReadFileState::File { file: Some(self) },
                    size,
                    cursor,
                    callback: read_file,
                },
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
            },
            Future {
                #[pin]
                fut: Fut,
            },
        }
    }

    async fn read_file(
        file: File,
        capacity: usize,
        cursor: u64,
    ) -> (File, BufResult<usize, Vec<u8>>) {
        let buf = Vec::with_capacity(capacity);

        let buf_res = file.read_at(buf, cursor).await;

        (file, buf_res)
    }

    impl<F, Fut> Stream for BytesStream<F, Fut>
    where
        F: Fn(File, usize, u64) -> Fut,
        Fut: Future<Output = (File, BufResult<usize, Vec<u8>>)> + 'static,
    {
        type Item = std::io::Result<Bytes>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut this = self.as_mut().project();

            match this.state.as_mut().project() {
                ReadFileStateProj::File { file } => {
                    let cursor = *this.cursor;
                    let max_size = *this.size - *this.cursor;

                    if max_size == 0 {
                        return Poll::Ready(None);
                    }

                    let capacity = max_size.min(65_356) as usize;
                    let file = file.take().unwrap();

                    let fut = (this.callback)(file, capacity, cursor);

                    this.state.project_replace(ReadFileState::Future { fut });
                    self.poll_next(cx)
                }
                ReadFileStateProj::Future { fut } => match fut.poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready((file, (Ok(n), mut buf))) => {
                        this.state
                            .project_replace(ReadFileState::File { file: Some(file) });

                        let _ = buf.split_off(n);
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

            let mut source_vec = Vec::new();
            source.read_to_end(&mut source_vec).unwrap();

            let mut dest_vec = Vec::new();
            dest.read_to_end(&mut dest_vec).unwrap();

            drop(dest);
            std::fs::remove_file(tmp).unwrap();

            assert_eq!(source_vec.len(), dest_vec.len());
            assert_eq!(source_vec, dest_vec);
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

            let mut source_vec = Vec::new();
            source.read_to_end(&mut source_vec).unwrap();

            let mut dest_vec = Vec::new();
            dest.read_to_end(&mut dest_vec).unwrap();

            drop(dest);
            std::fs::remove_file(tmp).unwrap();

            assert_eq!(source_vec.len(), dest_vec.len());
            assert_eq!(source_vec, dest_vec);
        }
    }
}
