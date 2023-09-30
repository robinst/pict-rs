mod metrics;
mod payload;

use actix_web::{
    dev::{Service, ServiceRequest, Transform},
    http::StatusCode,
    rt::time::Timeout,
    HttpResponse, ResponseError,
};
use std::{
    future::{ready, Future, Ready},
    pin::Pin,
    task::{Context, Poll},
};

use crate::future::WithTimeout;

pub(crate) use self::metrics::Metrics;
pub(crate) use self::payload::Payload;

pub(crate) struct Deadline;
pub(crate) struct DeadlineMiddleware<S> {
    inner: S,
}

#[derive(Debug)]
struct DeadlineExceeded;

pin_project_lite::pin_project! {
    pub(crate) struct DeadlineFuture<F> {
        #[pin]
        inner: DeadlineFutureInner<F>,
    }
}

pin_project_lite::pin_project! {
    #[project = DeadlineFutureInnerProj]
    #[project_replace = DeadlineFutureInnerProjReplace]
    enum DeadlineFutureInner<F> {
        Timed {
            #[pin]
            timeout: Timeout<F>,
        },
        Untimed {
            #[pin]
            future: F,
        },
    }
}

pub(crate) struct Internal(pub(crate) Option<String>);
pub(crate) struct InternalMiddleware<S>(Option<String>, S);
#[derive(Clone, Debug, thiserror::Error)]
#[error("Invalid API Key")]
pub(crate) struct ApiError;

pin_project_lite::pin_project! {
    #[project = InternalFutureProj]
    #[project_replace = InternalFutureProjReplace]
    pub(crate) enum InternalFuture<F> {
        Internal {
            #[pin]
            future: F,
        },
        Error {
            error: Option<ApiError>,
        },
    }
}

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
                let deadline = time::OffsetDateTime::from_unix_timestamp_nanos(
                    deadline.to_str().ok()?.parse().ok()?,
                )
                .ok()?;
                let now = time::OffsetDateTime::now_utc();

                if now < deadline {
                    Some((deadline - now).try_into().ok()?)
                } else {
                    Some(std::time::Duration::from_secs(0))
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
                serde_json::to_string(
                    &serde_json::json!({ "msg": self.to_string(), "code": "request-timeout" }),
                )
                .unwrap_or_else(|_| {
                    r#"{"msg":"request timeout","code":"request-timeout"}"#.to_string()
                }),
            )
    }
}

impl<F> DeadlineFuture<F>
where
    F: Future,
{
    fn new(future: F, timeout: Option<std::time::Duration>) -> Self {
        DeadlineFuture {
            inner: match timeout {
                Some(duration) => DeadlineFutureInner::Timed {
                    timeout: future.with_timeout(duration),
                },
                None => DeadlineFutureInner::Untimed { future },
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

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut().project();

        match this.inner.project() {
            DeadlineFutureInnerProj::Timed { timeout } => timeout.poll(cx).map(|res| match res {
                Ok(res) => res.map_err(actix_web::Error::from),
                Err(_) => Err(DeadlineExceeded.into()),
            }),
            DeadlineFutureInnerProj::Untimed { future } => future
                .poll(cx)
                .map(|res| res.map_err(actix_web::Error::from)),
        }
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
    type Future = InternalFuture<S::Future>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.1.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        if let Some(value) = req.headers().get("x-api-token") {
            if let (Ok(header), Some(api_key)) = (value.to_str(), &self.0) {
                if header == api_key {
                    return InternalFuture::Internal {
                        future: self.1.call(req),
                    };
                }
            }
        }

        InternalFuture::Error {
            error: Some(ApiError),
        }
    }
}

impl<F, T, E> Future for InternalFuture<F>
where
    F: Future<Output = Result<T, E>>,
    E: From<ApiError>,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.as_mut().project() {
            InternalFutureProj::Internal { future } => future.poll(cx),
            InternalFutureProj::Error { error } => Poll::Ready(Err(error.take().unwrap().into())),
        }
    }
}
