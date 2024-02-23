use crate::{
    details::Details,
    error::{Error, UploadError},
    repo::Hash,
};

use dashmap::{mapref::entry::Entry, DashMap};
use flume::{r#async::RecvFut, Receiver, Sender};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::Span;

type OutcomeReceiver = Receiver<(Details, Arc<str>)>;

type ProcessMapKey = (Hash, PathBuf);

type ProcessMapInner = DashMap<ProcessMapKey, OutcomeReceiver>;

#[derive(Debug, Default, Clone)]
pub(crate) struct ProcessMap {
    process_map: Arc<ProcessMapInner>,
}

impl ProcessMap {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) async fn process<Fut>(
        &self,
        hash: Hash,
        path: PathBuf,
        fut: Fut,
    ) -> Result<(Details, Arc<str>), Error>
    where
        Fut: Future<Output = Result<(Details, Arc<str>), Error>>,
    {
        let key = (hash.clone(), path.clone());

        let (sender, receiver) = flume::bounded(1);

        let entry = self.process_map.entry(key.clone());

        let (state, span) = match entry {
            Entry::Vacant(vacant) => {
                vacant.insert(receiver);

                let span = tracing::info_span!(
                    "Processing image",
                    hash = ?hash,
                    path = ?path,
                    completed = &tracing::field::Empty,
                );

                metrics::counter!(crate::init_metrics::PROCESS_MAP_INSERTED).increment(1);

                (CancelState::Sender { sender }, span)
            }
            Entry::Occupied(receiver) => {
                let span = tracing::info_span!(
                    "Waiting for processed image",
                    hash = ?hash,
                    path = ?path,
                );

                let receiver = receiver.get().clone().into_recv_async();

                (CancelState::Receiver { receiver }, span)
            }
        };

        CancelSafeProcessor {
            cancel_token: CancelToken {
                span,
                key,
                state,
                process_map: self.clone(),
            },
            fut,
        }
        .await
    }

    fn remove(&self, key: &ProcessMapKey) -> Option<OutcomeReceiver> {
        self.process_map.remove(key).map(|(_, v)| v)
    }
}

struct CancelToken {
    span: Span,
    key: ProcessMapKey,
    state: CancelState,
    process_map: ProcessMap,
}

enum CancelState {
    Sender {
        sender: Sender<(Details, Arc<str>)>,
    },
    Receiver {
        receiver: RecvFut<'static, (Details, Arc<str>)>,
    },
}

impl CancelState {
    const fn is_sender(&self) -> bool {
        matches!(self, Self::Sender { .. })
    }
}

pin_project_lite::pin_project! {
    struct CancelSafeProcessor<F> {
        cancel_token: CancelToken,

        #[pin]
        fut: F,
    }
}

impl<F> Future for CancelSafeProcessor<F>
where
    F: Future<Output = Result<(Details, Arc<str>), Error>>,
{
    type Output = Result<(Details, Arc<str>), Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().project();

        let span = &this.cancel_token.span;
        let process_map = &this.cancel_token.process_map;
        let state = &mut this.cancel_token.state;
        let key = &this.cancel_token.key;
        let fut = this.fut;

        span.in_scope(|| match state {
            CancelState::Sender { sender } => {
                let res = std::task::ready!(fut.poll(cx));

                if process_map.remove(key).is_some() {
                    metrics::counter!(crate::init_metrics::PROCESS_MAP_REMOVED).increment(1);
                }

                if let Ok(tup) = &res {
                    let _ = sender.try_send(tup.clone());
                }

                Poll::Ready(res)
            }
            CancelState::Receiver { ref mut receiver } => Pin::new(receiver)
                .poll(cx)
                .map(|res| res.map_err(|_| UploadError::Canceled.into())),
        })
    }
}

impl Drop for CancelToken {
    fn drop(&mut self) {
        if self.state.is_sender() {
            let completed = self.process_map.remove(&self.key).is_none();
            self.span.record("completed", completed);

            if !completed {
                metrics::counter!(crate::init_metrics::PROCESS_MAP_REMOVED).increment(1);
            }
        }
    }
}
