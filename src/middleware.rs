use actix_rt::time::Timeout;
use actix_web::{
    dev::{Service, ServiceRequest, Transform},
    http::StatusCode,
    HttpResponse, ResponseError,
};
use futures_util::future::LocalBoxFuture;
use std::{
    future::{ready, Future, Ready},
    pin::Pin,
    task::{Context, Poll},
};
use tracing_futures::{Instrument, Instrumented};
use uuid::Uuid;

pub(crate) struct Deadline;
pub(crate) struct DeadlineMiddleware<S> {
    inner: S,
}

#[derive(Debug)]
struct DeadlineExceeded;

enum DeadlineFutureInner<F> {
    Timed(Pin<Box<Timeout<F>>>),
    Untimed(Pin<Box<F>>),
}
pub(crate) struct DeadlineFuture<F> {
    inner: DeadlineFutureInner<F>,
}

pub(crate) struct Tracing;

pub(crate) struct TracingMiddleware<S> {
    inner: S,
}

pub(crate) struct Internal(pub(crate) Option<String>);
pub(crate) struct InternalMiddleware<S>(Option<String>, S);
#[derive(Clone, Debug, thiserror::Error)]
#[error("Invalid API Key")]
struct ApiError;

impl ResponseError for ApiError {
    fn status_code(&self) -> StatusCode {
        StatusCode::UNAUTHORIZED
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code())
            .content_type("application/json")
            .body(
                serde_json::to_string(&serde_json::json!({ "msg": self.to_string() }))
                    .unwrap_or_else(|_| r#"{"msg":"unauthorized"}"#.to_string()),
            )
    }
}

impl<S> Transform<S, ServiceRequest> for Deadline
where
    S: Service<ServiceRequest>,
    S::Future: 'static,
    actix_web::Error: From<S::Error>,
{
    type Response = S::Response;
    type Error = actix_web::Error;
    type InitError = ();
    type Transform = DeadlineMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(DeadlineMiddleware { inner: service }))
    }
}

impl<S> Service<ServiceRequest> for DeadlineMiddleware<S>
where
    S: Service<ServiceRequest>,
    S::Future: 'static,
    actix_web::Error: From<S::Error>,
{
    type Response = S::Response;
    type Error = actix_web::Error;
    type Future = DeadlineFuture<S::Future>;

    fn poll_ready(&self, cx: &mut core::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map(|res| res.map_err(actix_web::Error::from))
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let duration = req
            .headers()
            .get("X-Request-Deadline")
            .and_then(|deadline| {
                use std::convert::TryInto;
                let deadline = time::OffsetDateTime::from_unix_timestamp_nanos(
                    deadline.to_str().ok()?.parse().ok()?,
                )
                .ok()?;
                let now = time::OffsetDateTime::now_utc();

                if now < deadline {
                    Some((deadline - now).try_into().ok()?)
                } else {
                    None
                }
            });
        DeadlineFuture::new(self.inner.call(req), duration)
    }
}

impl std::fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Deadline exceeded")
    }
}

impl std::error::Error for DeadlineExceeded {}
impl actix_web::error::ResponseError for DeadlineExceeded {
    fn status_code(&self) -> StatusCode {
        StatusCode::REQUEST_TIMEOUT
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code())
            .content_type("application/json")
            .body(
                serde_json::to_string(&serde_json::json!({ "msg": self.to_string() }))
                    .unwrap_or_else(|_| r#"{"msg":"request timeout"}"#.to_string()),
            )
    }
}

impl<F> DeadlineFuture<F>
where
    F: Future,
{
    fn new(inner: F, timeout: Option<std::time::Duration>) -> Self {
        DeadlineFuture {
            inner: match timeout {
                Some(duration) => {
                    DeadlineFutureInner::Timed(Box::pin(actix_rt::time::timeout(duration, inner)))
                }
                None => DeadlineFutureInner::Untimed(Box::pin(inner)),
            },
        }
    }
}

impl<F, R, E> Future for DeadlineFuture<F>
where
    F: Future<Output = Result<R, E>>,
    actix_web::Error: From<E>,
{
    type Output = Result<R, actix_web::Error>;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.inner {
            DeadlineFutureInner::Timed(ref mut fut) => {
                Pin::new(fut).poll(cx).map(|res| match res {
                    Ok(res) => res.map_err(actix_web::Error::from),
                    Err(_) => Err(DeadlineExceeded.into()),
                })
            }
            DeadlineFutureInner::Untimed(ref mut fut) => Pin::new(fut)
                .poll(cx)
                .map(|res| res.map_err(actix_web::Error::from)),
        }
    }
}

impl<S, Request> Transform<S, Request> for Tracing
where
    S: Service<Request>,
    S::Future: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type InitError = ();
    type Transform = TracingMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(TracingMiddleware { inner: service }))
    }
}

impl<S, Request> Service<Request> for TracingMiddleware<S>
where
    S: Service<Request>,
    S::Future: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Instrumented<S::Future>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&self, req: Request) -> Self::Future {
        let uuid = Uuid::new_v4();

        self.inner
            .call(req)
            .instrument(tracing::info_span!("request", ?uuid))
    }
}

impl<S> Transform<S, ServiceRequest> for Internal
where
    S: Service<ServiceRequest, Error = actix_web::Error>,
    S::Future: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type InitError = ();
    type Transform = InternalMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(InternalMiddleware(self.0.clone(), service)))
    }
}

impl<S> Service<ServiceRequest> for InternalMiddleware<S>
where
    S: Service<ServiceRequest, Error = actix_web::Error>,
    S::Future: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = LocalBoxFuture<'static, Result<S::Response, S::Error>>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.1.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        if let Some(value) = req.headers().get("x-api-token") {
            if value.to_str().is_ok() && value.to_str().ok() == self.0.as_deref() {
                let fut = self.1.call(req);
                return Box::pin(async move { fut.await });
            }
        }

        Box::pin(async move { Err(ApiError.into()) })
    }
}
