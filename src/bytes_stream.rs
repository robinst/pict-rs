use actix_web::web::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use std::{
    collections::{vec_deque::IntoIter, VecDeque},
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

    pub(crate) fn into_io_stream(self) -> impl Stream<Item = std::io::Result<Bytes>> + Unpin {
        self.map(|bytes| Ok(bytes))
    }

    pub(crate) fn into_bytes(self) -> Bytes {
        let mut buf = BytesMut::with_capacity(self.total_len);

        for bytes in self.inner {
            buf.extend_from_slice(&bytes);
        }

        buf.freeze()
    }
}

impl Stream for BytesStream {
    type Item = Bytes;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().inner.pop_front())
    }
}

impl IntoIterator for BytesStream {
    type Item = Bytes;
    type IntoIter = IntoIter<Bytes>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}
