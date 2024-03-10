use actix_web::web::Bytes;
use futures_core::Stream;
use std::collections::{vec_deque::IntoIter, VecDeque};
use streem::IntoStreamer;

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
            crate::sync::cooperate().await;
        }

        tracing::debug!(
            "BytesStream with {} chunks, avg length {}",
            bs.chunks_len(),
            bs.len() / bs.chunks_len()
        );

        Ok(bs)
    }

    pub(crate) fn chunks_len(&self) -> usize {
        self.inner.len()
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

    pub(crate) fn into_io_stream(self) -> impl Stream<Item = std::io::Result<Bytes>> {
        streem::from_fn(move |yielder| async move {
            for bytes in self {
                crate::sync::cooperate().await;
                yielder.yield_ok(bytes).await;
            }
        })
    }
}

impl IntoIterator for BytesStream {
    type Item = Bytes;
    type IntoIter = IntoIter<Bytes>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}
