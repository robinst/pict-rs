use std::{
    future::Future,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

static NOOP_WAKER: OnceLock<std::task::Waker> = OnceLock::new();

fn noop_waker() -> &'static std::task::Waker {
    NOOP_WAKER.get_or_init(|| std::task::Waker::from(Arc::new(NoopWaker)))
}

struct NoopWaker;
impl std::task::Wake for NoopWaker {
    fn wake(self: std::sync::Arc<Self>) {}
    fn wake_by_ref(self: &std::sync::Arc<Self>) {}
}

pub(crate) type LocalBoxFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + 'a>>;

pub(crate) trait NowOrNever: Future {
    fn now_or_never(self) -> Option<Self::Output>
    where
        Self: Sized,
    {
        let fut = std::pin::pin!(self);

        let mut cx = std::task::Context::from_waker(noop_waker());

        match fut.poll(&mut cx) {
            std::task::Poll::Pending => None,
            std::task::Poll::Ready(out) => Some(out),
        }
    }
}

pub(crate) trait WithTimeout: Future {
    fn with_timeout(self, duration: Duration) -> tokio::time::Timeout<Self>
    where
        Self: Sized,
    {
        tokio::time::timeout(duration, self)
    }
}

pub(crate) trait WithMetrics: Future {
    fn with_metrics(self, name: &'static str) -> MetricsFuture<Self>
    where
        Self: Sized,
    {
        MetricsFuture {
            future: self,
            metrics: Metrics {
                name,
                start: Instant::now(),
                complete: false,
            },
        }
    }
}

impl<F> NowOrNever for F where F: Future {}
impl<F> WithMetrics for F where F: Future {}
impl<F> WithTimeout for F where F: Future {}

pin_project_lite::pin_project! {
    pub(crate) struct MetricsFuture<F> {
        #[pin]
        future: F,

        metrics: Metrics,
    }
}

struct Metrics {
    name: &'static str,
    start: Instant,
    complete: bool,
}

impl<F> Future for MetricsFuture<F>
where
    F: Future,
{
    type Output = F::Output;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();

        let out = std::task::ready!(this.future.poll(cx));

        this.metrics.complete = true;

        std::task::Poll::Ready(out)
    }
}

impl Drop for Metrics {
    fn drop(&mut self) {
        metrics::histogram!(self.name, self.start.elapsed().as_secs_f64(), "complete" => self.complete.to_string());
    }
}
