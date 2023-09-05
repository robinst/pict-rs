use std::{
    future::Future,
    time::{Duration, Instant},
};

pub(crate) type LocalBoxFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + 'a>>;

pub(crate) trait WithTimeout: Future {
    fn with_timeout(self, duration: Duration) -> actix_rt::time::Timeout<Self>
    where
        Self: Sized,
    {
        actix_rt::time::timeout(duration, self)
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
