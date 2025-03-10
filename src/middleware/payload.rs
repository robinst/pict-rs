use std::{
    future::{ready, Ready},
    rc::Rc,
    time::{Duration, Instant},
};

use actix_web::{
    dev::{Service, ServiceRequest, Transform},
    HttpMessage,
};
use streem::IntoStreamer;
use tokio::task::JoinSet;

use crate::{future::NowOrNever, stream::LocalBoxStream};

const LIMIT: usize = 256;

struct MetricsGuard {
    start: Instant,
    armed: bool,
}

impl MetricsGuard {
    fn guard() -> Self {
        metrics::counter!(crate::init_metrics::PAYLOAD_DRAIN_START).increment(1);

        MetricsGuard {
            start: Instant::now(),
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for MetricsGuard {
    fn drop(&mut self) {
        metrics::counter!(crate::init_metrics::PAYLOAD_DRAIN_END, "completed" => (!self.armed).to_string())
            .increment(1);

        metrics::histogram!(crate::init_metrics::PAYLOAD_DRAIN_DURATION, "completed" => (!self.armed).to_string())
            .record(self.start.elapsed().as_secs_f64());
    }
}

async fn drain(mut rx: tokio::sync::mpsc::Receiver<actix_web::dev::Payload>) {
    let mut set = JoinSet::new();

    while let Some(payload) = rx.recv().await {
        tracing::trace!("drain: looping");

        // draining a payload is a best-effort task - if we can't collect in 2 minutes we bail
        let guard = MetricsGuard::guard();
        set.spawn_local(tokio::time::timeout(Duration::from_secs(120), async move {
            let mut streamer = payload.into_streamer();
            while streamer.next().await.is_some() {
                tracing::trace!("drain drop bytes: looping");
            }
            guard.disarm();
        }));

        let mut count = 0;

        // drain completed tasks
        while set.join_next().now_or_never().is_some() {
            tracing::trace!("drain join now: looping");

            count += 1;
        }

        // if we're past the limit, wait for completions
        while set.len() > LIMIT {
            tracing::trace!("drain join await: looping");

            if set.join_next().await.is_some() {
                count += 1;
            }
        }

        if count > 0 {
            tracing::debug!("Drained {count} dropped payloads");
        }
    }

    // drain set
    while set.join_next().await.is_some() {
        tracing::trace!("drain join await cleanup: looping");
    }
}

#[derive(Clone)]
struct DrainHandle(Option<Rc<tokio::task::JoinHandle<()>>>);

pub(crate) struct Payload {
    sender: tokio::sync::mpsc::Sender<actix_web::dev::Payload>,
    handle: DrainHandle,
}
pub(crate) struct PayloadMiddleware<S> {
    inner: S,
    sender: tokio::sync::mpsc::Sender<actix_web::dev::Payload>,
    _handle: DrainHandle,
}

pub(crate) struct PayloadStream {
    inner: Option<actix_web::dev::Payload>,
    sender: tokio::sync::mpsc::Sender<actix_web::dev::Payload>,
}

impl DrainHandle {
    fn new(handle: tokio::task::JoinHandle<()>) -> Self {
        Self(Some(Rc::new(handle)))
    }
}

impl Payload {
    pub(crate) fn new() -> Self {
        let (tx, rx) = crate::sync::channel(LIMIT);

        let handle = DrainHandle::new(crate::sync::spawn("drain-payloads", drain(rx)));

        Payload { sender: tx, handle }
    }
}

impl Drop for DrainHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take().and_then(Rc::into_inner) {
            handle.abort();
        }
    }
}

impl Drop for PayloadStream {
    fn drop(&mut self) {
        if let Some(payload) = self.inner.take() {
            tracing::debug!("Dropped unclosed payload, draining");
            if self.sender.try_send(payload).is_err() {
                metrics::counter!(crate::init_metrics::PAYLOAD_DRAIN_FAIL_SEND).increment(1);
                tracing::error!("Failed to send unclosed payload for draining");
            }
        }
    }
}

impl futures_core::Stream for PayloadStream {
    type Item = Result<actix_web::web::Bytes, actix_web::error::PayloadError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if let Some(inner) = self.inner.as_mut() {
            let opt = std::task::ready!(std::pin::Pin::new(inner).poll_next(cx));

            if opt.is_none() {
                self.inner.take();
            }

            std::task::Poll::Ready(opt)
        } else {
            std::task::Poll::Ready(None)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if let Some(inner) = self.inner.as_ref() {
            inner.size_hint()
        } else {
            (0, Some(0))
        }
    }
}

impl<S> Transform<S, ServiceRequest> for Payload
where
    S: Service<ServiceRequest>,
    S::Future: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type InitError = ();
    type Transform = PayloadMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(PayloadMiddleware {
            inner: service,
            sender: self.sender.clone(),
            _handle: self.handle.clone(),
        }))
    }
}

impl<S> Service<ServiceRequest> for PayloadMiddleware<S>
where
    S: Service<ServiceRequest>,
    S::Future: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &self,
        ctx: &mut core::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(ctx)
    }

    fn call(&self, mut req: ServiceRequest) -> Self::Future {
        let payload = req.take_payload();

        if !matches!(payload, actix_web::dev::Payload::None) {
            let payload: LocalBoxStream<'static, _> = Box::pin(PayloadStream {
                inner: Some(payload),
                sender: self.sender.clone(),
            });
            req.set_payload(payload.into());
        }

        self.inner.call(req)
    }
}
