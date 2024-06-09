use std::path::Path;

use futures_core::Stream;
use tokio_util::bytes::Bytes;

#[cfg(feature = "io-uring")]
pub(crate) use io_uring::File;

#[cfg(not(feature = "io-uring"))]
pub(crate) use tokio_file::File;

use crate::future::WithPollTimer;

pub(crate) async fn write_from_stream(
    path: impl AsRef<Path>,
    stream: impl Stream<Item = std::io::Result<Bytes>>,
) -> std::io::Result<()> {
    let mut file = File::create(path).with_poll_timer("create-file").await?;
    file.write_from_stream(stream)
        .with_poll_timer("write-from-stream")
        .await?;
    file.close().await?;
    Ok(())
}

#[cfg(not(feature = "io-uring"))]
mod tokio_file {
    use crate::{future::WithPollTimer, store::file_store::FileError, Either};
    use actix_web::web::{Bytes, BytesMut};
    use futures_core::Stream;
    use std::{io::SeekFrom, path::Path};
    use streem::IntoStreamer;
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    use tokio_util::{
        bytes::Buf,
        codec::{BytesCodec, FramedRead},
    };

    pub(crate) struct File {
        inner: tokio::fs::File,
    }

    impl File {
        pub(crate) async fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
            Ok(File {
                inner: tracing::trace_span!(parent: None, "Open File")
                    .in_scope(|| tokio::fs::File::open(path))
                    .await?,
            })
        }

        pub(crate) async fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
            Ok(File {
                inner: tracing::trace_span!(parent: None, "Create File")
                    .in_scope(|| tokio::fs::File::create(path))
                    .await?,
            })
        }

        pub(crate) async fn write_from_stream<S>(&mut self, stream: S) -> std::io::Result<()>
        where
            S: Stream<Item = std::io::Result<Bytes>>,
        {
            let stream = std::pin::pin!(stream);
            let mut stream = stream.into_streamer();

            while let Some(mut bytes) = stream.try_next().with_poll_timer("try-next").await? {
                tracing::trace!("write_from_stream: looping");

                while bytes.has_remaining() {
                    self.inner
                        .write_buf(&mut bytes)
                        .with_poll_timer("write-buf")
                        .await?;

                    crate::sync::cooperate().await;
                }
            }

            Ok(())
        }

        pub(crate) async fn close(self) -> std::io::Result<()> {
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

            Ok(crate::stream::map_ok(
                FramedRead::new(obj, BytesCodec::new()),
                BytesMut::freeze,
            ))
        }
    }
}

#[cfg(feature = "io-uring")]
mod io_uring {
    use crate::store::file_store::FileError;
    use actix_web::web::{Bytes, BytesMut};
    use futures_core::Stream;
    use std::{
        convert::TryInto,
        fs::Metadata,
        path::{Path, PathBuf},
    };
    use streem::IntoStreamer;
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
            tracing::debug!("Opening io-uring file: {:?}", path.as_ref());
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: tracing::trace_span!(parent: None, "Open File")
                    .in_scope(|| tokio_uring::fs::File::open(path))
                    .await?,
            })
        }

        pub(crate) async fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
            tracing::debug!("Creating io-uring file: {:?}", path.as_ref());
            Ok(File {
                path: path.as_ref().to_owned(),
                inner: tracing::trace_span!(parent: None, "Create File")
                    .in_scope(|| tokio_uring::fs::File::create(path))
                    .await?,
            })
        }

        async fn metadata(&self) -> std::io::Result<Metadata> {
            tokio::fs::metadata(&self.path).await
        }

        pub(crate) async fn write_from_stream<S>(&mut self, stream: S) -> std::io::Result<()>
        where
            S: Stream<Item = std::io::Result<Bytes>>,
        {
            let stream = std::pin::pin!(stream);
            let mut stream = stream.into_streamer();
            let mut cursor: u64 = 0;

            while let Some(res) = stream.next().await {
                tracing::trace!("write_from_stream while: looping");

                let buf = res?;

                let len = buf.len();
                let mut position: usize = 0;

                loop {
                    tracing::trace!("write_from_stream: looping");

                    if position == len {
                        break;
                    }

                    let (res, _buf) = self
                        .write_at(buf.slice(position..), cursor + (position as u64))
                        .await;

                    let n = res?;
                    if n == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }

                    position += n;
                }

                let len: u64 = len.try_into().unwrap();
                cursor += len;
            }

            self.inner.sync_all().await?;

            Ok(())
        }

        pub(crate) async fn close(self) -> std::io::Result<()> {
            self.inner.close().await
        }

        pub(crate) async fn read_to_stream(
            self,
            from_start: Option<u64>,
            len: Option<u64>,
        ) -> Result<impl Stream<Item = std::io::Result<Bytes>>, FileError> {
            Ok(bytes_stream(self, from_start, len))
        }

        async fn read_at<T: IoBufMut>(&self, buf: T, pos: u64) -> BufResult<usize, T> {
            self.inner.read_at(buf, pos).await
        }

        async fn write_at<T: IoBuf>(&self, buf: T, pos: u64) -> BufResult<usize, T> {
            self.inner.write_at(buf, pos).submit().await
        }
    }

    fn bytes_stream(
        file: File,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> impl Stream<Item = std::io::Result<Bytes>> {
        streem::try_from_fn(|yielder| async move {
            let file_size = file.metadata().await?.len();

            let mut cursor = from_start.unwrap_or(0);
            let remaining_size = file_size.saturating_sub(cursor);
            let read_until = len.unwrap_or(remaining_size) + cursor;

            let mut bytes = BytesMut::new();

            loop {
                tracing::trace!("bytes_stream: looping");

                let max_size = read_until.saturating_sub(cursor);

                if max_size == 0 {
                    break;
                }

                let capacity = max_size.min(65_356) as usize;

                if bytes.capacity() < capacity {
                    bytes.reserve(capacity - bytes.capacity());
                }

                let (result, mut buf_) = file.read_at(bytes, cursor).await;

                let n = match result {
                    Ok(n) => n,
                    Err(e) => return Err(e),
                };

                bytes = buf_.split_off(n);

                let n: u64 = n.try_into().map_err(|_| std::io::ErrorKind::Other)?;
                cursor += n;

                yielder.yield_ok(buf_.into()).await;
            }

            Ok(())
        })
    }

    #[cfg(test)]
    mod tests {
        use std::io::Read;
        use streem::IntoStreamer;
        use tokio::io::AsyncWriteExt;

        macro_rules! test_async {
            ($fut:expr) => {
                actix_web::rt::System::new()
                    .block_on(async move { crate::sync::spawn("tests", $fut).await.unwrap() })
            };
        }

        const EARTH_GIF: &'static str = "client-examples/earth.gif";

        #[test]
        fn read() {
            let tmp = "/tmp/read-test";

            test_async!(async move {
                let file = super::File::open(EARTH_GIF).await.unwrap();
                let mut tmp_file = tokio::fs::File::create(tmp).await.unwrap();

                let stream = file.read_to_stream(None, None).await.unwrap();
                let stream = std::pin::pin!(stream);
                let mut stream = stream.into_streamer();

                while let Some(mut bytes) = stream.try_next().await.unwrap() {
                    tmp_file.write_all_buf(&mut bytes).await.unwrap();
                }
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

            test_async!(async move {
                let file = tokio::fs::File::open(EARTH_GIF).await.unwrap();
                let mut tmp_file = super::File::create(tmp).await.unwrap();
                tmp_file
                    .write_from_stream(tokio_util::io::ReaderStream::new(file))
                    .await
                    .unwrap();
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
