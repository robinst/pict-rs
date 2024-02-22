use actix_web::web::Bytes;
use futures_core::Stream;
use std::{
    collections::{vec_deque::IntoIter, VecDeque},
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
};
use streem::IntoStreamer;
use tokio::io::AsyncRead;

#[derive(Clone, Debug)]
pub(crate) struct BytesStream {
    inner: VecDeque<Bytes>,
    total_len: usize,
}

impl BytesStream {
    pub(crate) fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            total_len: 0,
        }
    }

    #[tracing::instrument(skip(stream))]
    pub(crate) async fn try_from_stream<S, E>(stream: S) -> Result<Self, E>
    where
        S: Stream<Item = Result<Bytes, E>>,
    {
        let stream = std::pin::pin!(stream);
        let mut stream = stream.into_streamer();
        let mut bs = Self::new();

        while let Some(bytes) = stream.try_next().await? {
            tracing::trace!("try_from_stream: looping");
            bs.add_bytes(bytes);
        }

        Ok(bs)
    }

    pub(crate) fn add_bytes(&mut self, bytes: Bytes) {
        self.total_len += bytes.len();
        self.inner.push_back(bytes);
    }

    pub(crate) fn len(&self) -> usize {
        self.total_len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.total_len == 0
    }

    pub(crate) fn into_reader(self) -> BytesReader {
        BytesReader {
            index: 0,
            inner: self.inner,
        }
    }

    pub(crate) fn into_io_stream(self) -> IoStream {
        IoStream { inner: self.inner }
    }
}

pub(crate) struct IoStream {
    inner: VecDeque<Bytes>,
}

pub(crate) struct BytesReader {
    index: usize,
    inner: VecDeque<Bytes>,
}

impl IntoIterator for BytesStream {
    type Item = Bytes;
    type IntoIter = IntoIter<Bytes>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl Stream for BytesStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().inner.pop_front().map(Ok))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.inner.len(), Some(self.inner.len()))
    }
}

impl Stream for IoStream {
    type Item = std::io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().inner.pop_front().map(Ok))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.inner.len(), Some(self.inner.len()))
    }
}

impl AsyncRead for BytesReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        while buf.remaining() > 0 {
            if let Some(bytes) = self.inner.front() {
                if self.index == bytes.len() {
                    self.inner.pop_front();
                    self.index = 0;
                    continue;
                }

                let upper_bound = (self.index + buf.remaining()).min(bytes.len());

                let slice = &bytes[self.index..upper_bound];

                buf.put_slice(slice);
                self.index += slice.len();
            } else {
                break;
            }
        }

        Poll::Ready(Ok(()))
    }
}

impl From<Bytes> for BytesStream {
    fn from(value: Bytes) -> Self {
        let mut bs = BytesStream::new();
        bs.add_bytes(value);
        bs
    }
}
