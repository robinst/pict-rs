use actix_web::{
    body::MessageBody,
    web::{Bytes, BytesMut},
};
use futures_util::Stream;
use std::{
    collections::{vec_deque::IntoIter, VecDeque},
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
};

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

    pub(crate) fn add_bytes(&mut self, bytes: Bytes) {
        self.total_len += bytes.len();
        self.inner.push_back(bytes);
    }

    pub(crate) fn len(&self) -> usize {
        self.total_len
    }

    pub(crate) fn into_bytes(self) -> Bytes {
        let mut buf = BytesMut::with_capacity(self.total_len);

        for bytes in self.inner {
            buf.extend_from_slice(&bytes);
        }

        buf.freeze()
    }
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
