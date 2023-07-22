use crate::{
    details::Details,
    error::{Error, UploadError},
};
use actix_web::web;
use dashmap::{mapref::entry::Entry, DashMap};
use flume::{r#async::RecvFut, Receiver, Sender};
use once_cell::sync::Lazy;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};
use tracing::Span;

type OutcomeReceiver = Receiver<(Details, web::Bytes)>;

type ProcessMapKey = (Vec<u8>, PathBuf);

type ProcessMap = DashMap<ProcessMapKey, OutcomeReceiver>;

static PROCESS_MAP: Lazy<ProcessMap> = Lazy::new(DashMap::new);

struct CancelToken {
    span: Span,
    key: ProcessMapKey,
    state: CancelState,
}

enum CancelState {
    Sender {
        sender: Sender<(Details, web::Bytes)>,
    },
    Receiver {
        receiver: RecvFut<'static, (Details, web::Bytes)>,
    },
}

impl CancelState {
    const fn is_sender(&self) -> bool {
        matches!(self, Self::Sender { .. })
    }
}

pin_project_lite::pin_project! {
    pub(super) struct CancelSafeProcessor<F> {
        cancel_token: CancelToken,

        #[pin]
        fut: F,
    }
}

impl<F> CancelSafeProcessor<F>
where
    F: Future<Output = Result<(Details, web::Bytes), Error>>,
{
    pub(super) fn new(hash: &[u8], path: PathBuf, fut: F) -> Self {
        let key = (hash.to_vec(), path.clone());

        let (sender, receiver) = flume::bounded(1);

        let entry = PROCESS_MAP.entry(key.clone());

        let (state, span) = match entry {
            Entry::Vacant(vacant) => {
                vacant.insert(receiver);
                let span = tracing::info_span!(
                    "Processing image",
                    hash = &tracing::field::debug(&hex::encode(hash)),
                    path = &tracing::field::debug(&path),
                    completed = &tracing::field::Empty,
                );
                (CancelState::Sender { sender }, span)
            }
            Entry::Occupied(receiver) => {
                let span = tracing::info_span!(
                    "Waiting for processed image",
                    hash = &tracing::field::debug(&hex::encode(hash)),
                    path = &tracing::field::debug(&path),
                );
                (
                    CancelState::Receiver {
                        receiver: receiver.get().clone().into_recv_async(),
                    },
                    span,
                )
            }
        };

        CancelSafeProcessor {
            cancel_token: CancelToken { span, key, state },
            fut,
        }
    }
}

impl<F> Future for CancelSafeProcessor<F>
where
    F: Future<Output = Result<(Details, web::Bytes), Error>>,
{
    type Output = Result<(Details, web::Bytes), Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().project();

        let span = &this.cancel_token.span;
        let state = &mut this.cancel_token.state;
        let key = &this.cancel_token.key;
        let fut = this.fut;

        span.in_scope(|| match state {
            CancelState::Sender { sender } => fut.poll(cx).map(|res| {
                PROCESS_MAP.remove(key);

                if let Ok(tup) = &res {
                    let _ = sender.try_send(tup.clone());
                }

                res
            }),
            CancelState::Receiver { ref mut receiver } => Pin::new(receiver)
                .poll(cx)
                .map(|res| res.map_err(|_| UploadError::Canceled.into())),
        })
    }
}

impl Drop for CancelToken {
    fn drop(&mut self) {
        if self.state.is_sender() {
            let completed = PROCESS_MAP.remove(&self.key).is_none();
            self.span.record("completed", completed);
        }
    }
}
