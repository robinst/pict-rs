use actix_web::{
    body::MessageBody,
    web::{Bytes, BytesMut},
};
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

    pub(crate) async fn try_from_stream<S, E>(stream: S) -> Result<Self, E>
    where
        S: Stream<Item = Result<Bytes, E>>,
    {
        let stream = std::pin::pin!(stream);
        let mut stream = stream.into_streamer();
        let mut bs = Self::new();

        while let Some(bytes) = stream.try_next().await? {
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
        self.total_len > 0
    }

    fn into_bytes(mut self) -> Bytes {
        if self.inner.len() == 1 {
            return self.inner.pop_front().expect("Exactly one");
        }

        let mut buf = BytesMut::with_capacity(self.total_len);

        for bytes in self.inner {
            buf.extend_from_slice(&bytes);
        }

        buf.freeze()
    }

    pub(crate) fn into_reader(self) -> BytesReader {
        BytesReader {
            index: 0,
            inner: self.inner,
        }
    }

    pub(crate) fn into_io_stream(self) -> IoStream {
        IoStream { stream: self }
    }
}

pub(crate) struct IoStream {
    stream: BytesStream,
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

impl MessageBody for BytesStream {
    type Error = std::io::Error;

    fn size(&self) -> actix_web::body::BodySize {
        if let Ok(len) = self.len().try_into() {
            actix_web::body::BodySize::Sized(len)
        } else {
            actix_web::body::BodySize::None
        }
    }

    fn poll_next(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Bytes, Self::Error>>> {
        Poll::Ready(self.get_mut().inner.pop_front().map(Ok))
    }

    fn try_into_bytes(self) -> Result<Bytes, Self>
    where
        Self: Sized,
    {
        Ok(self.into_bytes())
    }
}

impl Stream for BytesStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().inner.pop_front().map(Ok))
    }
}

impl Stream for IoStream {
    type Item = std::io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        MessageBody::poll_next(Pin::new(&mut self.get_mut().stream), cx)
    }
}

impl AsyncRead for BytesReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        while buf.remaining() > 0 {
            if self.index == self.inner[0].len() {
                self.inner.pop_front();
                self.index = 0;
            }

            if self.inner.is_empty() {
                break;
            }

            let upper_bound = (self.index + buf.remaining()).min(self.inner[0].len());

            let slice = &self.inner[0][self.index..upper_bound];

            buf.put_slice(slice);
            self.index += slice.len();
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
