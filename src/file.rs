use futures_util::stream::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[cfg(feature = "io-uring")]
pub(crate) use io_uring::File;

#[cfg(not(feature = "io-uring"))]
pub(crate) use tokio_file::File;

struct CrateError<S>(S);

impl<T, E, S> Stream for CrateError<S>
where
    S: Stream<Item = Result<T, E>> + Unpin,
    crate::error::Error: From<E>,
{
    type Item = Result<T, crate::error::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
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
        ) -> Result<
            impl Stream<Item = Result<Bytes, crate::error::Error>> + Unpin,
            crate::error::Error,
        > {
            let obj = match (from_start, len) {
                (Some(lower), Some(upper)) => {
                    self.inner.seek(SeekFrom::Start(lower)).await?;
                    Either::Left(self.inner.take(upper))
                }
                (None, Some(upper)) => Either::Left(self.inner.take(upper)),
                (Some(lower), None) => {
                    self.inner.seek(SeekFrom::Start(lower)).await?;
                    Either::Right(self.inner)
                }
                (None, None) => Either::Right(self.inner),
            };

            Ok(super::CrateError(BytesFreezer(FramedRead::new(
                obj,
                BytesCodec::new(),
            ))))
        }
    }

    struct BytesFreezer<S>(S);

    impl<S, E> Stream for BytesFreezer<S>
    where
        S: Stream<Item = Result<BytesMut, E>> + Unpin,
    {
        type Item = Result<Bytes, E>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::pin::Pin::new(&mut self.0)
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
        buf::{IoBuf, IoBufMut, Slice},
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

            Ok(())
        }

        pub(crate) async fn write_from_async_read<'a, R>(
            &'a mut self,
            mut reader: R,
        ) -> std::io::Result<()>
        where
            R: AsyncRead + Unpin,
        {
            let metadata = self.metadata().await?;
            let size = metadata.len();

            let mut cursor: u64 = 0;

            loop {
                let max_size = (size - cursor).min(65_536);
                let mut buf = Vec::with_capacity(max_size.try_into().unwrap());

                let n = (&mut reader).take(max_size).read_to_end(&mut buf).await?;

                if n == 0 {
                    break;
                }

                let mut buf: Slice<Vec<u8>> = buf.slice(..n);
                let mut position = 0;

                loop {
                    if position == buf.len() {
                        break;
                    }

                    let (res, slice) = self.write_at(buf.slice(position..), cursor).await;
                    position += res?;

                    buf = slice.into_inner();
                }

                let position: u64 = position.try_into().unwrap();
                cursor += position;
            }

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
        ) -> Result<
            impl Stream<Item = Result<Bytes, crate::error::Error>> + Unpin,
            crate::error::Error,
        > {
            let size = self.metadata().await?.len();

            let cursor = from_start.unwrap_or(0);
            let size = len.unwrap_or(size - cursor) + cursor;

            Ok(super::CrateError(BytesStream {
                file: Some(self),
                size,
                cursor,
                fut: None,
            }))
        }

        async fn read_at<T: IoBufMut>(&self, buf: T, pos: u64) -> BufResult<usize, T> {
            self.inner.read_at(buf, pos).await
        }

        async fn write_at<T: IoBuf>(&self, buf: T, pos: u64) -> BufResult<usize, T> {
            self.inner.write_at(buf, pos).await
        }
    }

    struct BytesStream {
        file: Option<File>,
        size: u64,
        cursor: u64,
        fut: Option<Pin<Box<dyn Future<Output = (File, BufResult<usize, Vec<u8>>)>>>>,
    }

    impl Stream for BytesStream {
        type Item = std::io::Result<Bytes>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut fut = if let Some(fut) = self.fut.take() {
                fut
            } else {
                let file = self.file.take().unwrap();

                if self.cursor == self.size {
                    return Poll::Ready(None);
                }

                let cursor = self.cursor;
                let max_size = self.size - self.cursor;

                Box::pin(async move {
                    let buf = Vec::with_capacity(max_size.try_into().unwrap());

                    let buf_res = file.read_at(buf, cursor).await;

                    (file, buf_res)
                })
            };

            match Pin::new(&mut fut).poll(cx) {
                Poll::Pending => {
                    self.fut = Some(fut);
                    Poll::Pending
                }
                Poll::Ready((file, (Ok(n), mut buf))) => {
                    self.file = Some(file);
                    let _ = buf.split_off(n);
                    let n: u64 = match n.try_into() {
                        Ok(n) => n,
                        Err(_) => return Poll::Ready(Some(Err(std::io::ErrorKind::Other.into()))),
                    };
                    self.cursor += n;

                    Poll::Ready(Some(Ok(Bytes::from(buf))))
                }
                Poll::Ready((_, (Err(e), _))) => Poll::Ready(Some(Err(e))),
            }
        }
    }
}
