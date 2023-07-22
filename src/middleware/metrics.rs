use actix_web::{
    body::MessageBody,
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    http::StatusCode,
    HttpResponse, ResponseError,
};
use std::{
    cell::RefCell,
    future::{ready, Future, Ready},
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

struct MetricsGuard {
    start: Instant,
    matched_path: Option<String>,
    armed: bool,
}

struct MetricsGuardWithStatus {
    start: Instant,
    matched_path: Option<String>,
    status: StatusCode,
}

impl MetricsGuard {
    fn new(matched_path: Option<String>) -> Self {
        metrics::increment_counter!("pict-rs.request.start", "path" => format!("{matched_path:?}"));

        Self {
            start: Instant::now(),
            matched_path,
            armed: true,
        }
    }

    fn with_status(mut self, status: StatusCode) -> MetricsGuardWithStatus {
        self.armed = false;

        MetricsGuardWithStatus {
            start: self.start,
            matched_path: self.matched_path.clone(),
            status,
        }
    }
}

impl Drop for MetricsGuard {
    fn drop(&mut self) {
        if self.armed {
            metrics::increment_counter!("pict-rs.request.complete", "path" => format!("{:?}", self.matched_path));
            metrics::histogram!("pict-rs.request.timings", self.start.elapsed().as_secs_f64(), "path" => format!("{:?}", self.matched_path))
        }
    }
}

impl Drop for MetricsGuardWithStatus {
    fn drop(&mut self) {
        metrics::increment_counter!("pict-rs.request.complete", "path" => format!("{:?}", self.matched_path), "status" => self.status.to_string());
        metrics::histogram!("pict-rs.request.timings", self.start.elapsed().as_secs_f64(), "path" => format!("{:?}", self.matched_path), "status" => self.status.to_string());
    }
}

pub(crate) struct Metrics;
pub(crate) struct MetricsMiddleware<S> {
    inner: S,
}

pub(crate) struct MetricsError {
    guard: RefCell<Option<MetricsGuard>>,
    inner: actix_web::Error,
}

pin_project_lite::pin_project! {
    pub(crate) struct MetricsFuture<F> {
        guard: Option<MetricsGuard>,

        #[pin]
        inner: F,
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct MetricsBody<B> {
        guard: Option<MetricsGuardWithStatus>,

        #[pin]
        inner: B,
    }
}

impl<S, B> Transform<S, ServiceRequest> for Metrics
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>>,
    S::Future: 'static,
    S::Error: Into<actix_web::Error>,
{
    type Response = ServiceResponse<MetricsBody<B>>;
    type Error = actix_web::Error;
    type InitError = ();
    type Transform = MetricsMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(MetricsMiddleware { inner: service }))
    }
}

impl<S, B> Service<ServiceRequest> for MetricsMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>>,
    S::Future: 'static,
    S::Error: Into<actix_web::Error>,
{
    type Response = ServiceResponse<MetricsBody<B>>;
    type Error = actix_web::Error;
    type Future = MetricsFuture<S::Future>;

    fn poll_ready(&self, ctx: &mut core::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        let res = std::task::ready!(self.inner.poll_ready(ctx));

        Poll::Ready(res.map_err(|e| {
            MetricsError {
                guard: RefCell::new(None),
                inner: e.into(),
            }
            .into()
        }))
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let matched_path = req.match_pattern();

        MetricsFuture {
            guard: Some(MetricsGuard::new(matched_path)),
            inner: self.inner.call(req),
        }
    }
}

impl<F, B, E> Future for MetricsFuture<F>
where
    F: Future<Output = Result<ServiceResponse<B>, E>>,
    E: Into<actix_web::Error>,
{
    type Output = Result<ServiceResponse<MetricsBody<B>>, actix_web::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match std::task::ready!(this.inner.poll(cx)) {
            Ok(response) => {
                let guard = this.guard.take();

                Poll::Ready(Ok(response.map_body(|head, inner| MetricsBody {
                    guard: guard.map(|guard| guard.with_status(head.status)),
                    inner,
                })))
            }
            Err(e) => {
                let guard = this.guard.take();

                Poll::Ready(Err(MetricsError {
                    guard: RefCell::new(guard),
                    inner: e.into(),
                }
                .into()))
            }
        }
    }
}

impl<B> MessageBody for MetricsBody<B>
where
    B: MessageBody,
{
    type Error = B::Error;

    fn size(&self) -> actix_web::body::BodySize {
        self.inner.size()
    }

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<actix_web::web::Bytes, Self::Error>>> {
        let this = self.project();

        let opt = std::task::ready!(this.inner.poll_next(cx));

        if opt.is_none() {
            this.guard.take();
        }

        Poll::Ready(opt)
    }
}

impl std::fmt::Debug for MetricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsError")
            .field("guard", &"Guard")
            .field("inner", &self.inner)
            .finish()
    }
}

impl std::fmt::Display for MetricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl std::error::Error for MetricsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source()
    }
}

impl ResponseError for MetricsError {
    fn status_code(&self) -> StatusCode {
        self.inner.as_response_error().status_code()
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        let guard = self.guard.borrow_mut().take();

        self.inner.error_response().map_body(|head, inner| {
            MetricsBody {
                guard: guard.map(|guard| guard.with_status(head.status)),
                inner,
            }
            .boxed()
        })
    }
}
