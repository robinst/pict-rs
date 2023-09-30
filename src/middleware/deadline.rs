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
    time::Duration,
};

use crate::future::WithTimeout;

pub(crate) struct Deadline;
pub(crate) struct DeadlineMiddleware<S> {
    inner: S,
}

#[derive(Debug, thiserror::Error)]
#[error("Deadline exceeded")]
struct DeadlineExceeded;

#[derive(Debug, thiserror::Error)]
enum ParseDeadlineError {
    #[error("Invalid header string")]
    HeaderString,

    #[error("Invalid deadline format")]
    HeaderFormat,

    #[error("Invalid deadline timestmap")]
    Timestamp,

    #[error("Invalid deadline duration")]
    Duration,
}

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
        Error {
            error: Option<ParseDeadlineError>,
        },
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
            .map(|deadline| {
                let deadline_str = deadline
                    .to_str()
                    .map_err(|_| ParseDeadlineError::HeaderString)?;

                let deadline_i128 = deadline_str
                    .parse()
                    .map_err(|_| ParseDeadlineError::HeaderFormat)?;

                let deadline = time::OffsetDateTime::from_unix_timestamp_nanos(deadline_i128)
                    .map_err(|_| ParseDeadlineError::Timestamp)?;

                let now = time::OffsetDateTime::now_utc();

                if now < deadline {
                    (deadline - now)
                        .try_into()
                        .map_err(|_| ParseDeadlineError::Duration)
                } else {
                    Ok(Duration::from_secs(0))
                }
            })
            .transpose();

        DeadlineFuture::new(self.inner.call(req), duration)
    }
}

impl ResponseError for DeadlineExceeded {
    fn status_code(&self) -> StatusCode {
        StatusCode::REQUEST_TIMEOUT
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code()).json(serde_json::json!({
            "msg": self.to_string(),
            "code": "request-timeout"
        }))
    }
}

impl ResponseError for ParseDeadlineError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        HttpResponse::build(self.status_code()).json(serde_json::json!({
            "msg": self.to_string(),
            "code": "parse-deadline"
        }))
    }
}

impl<F> DeadlineFuture<F>
where
    F: Future,
{
    fn new(future: F, timeout: Result<Option<Duration>, ParseDeadlineError>) -> Self {
        DeadlineFuture {
            inner: match timeout {
                Ok(Some(duration)) => DeadlineFutureInner::Timed {
                    timeout: future.with_timeout(duration),
                },
                Ok(None) => DeadlineFutureInner::Untimed { future },
                Err(e) => DeadlineFutureInner::Error { error: Some(e) },
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
            DeadlineFutureInnerProj::Error { error } => {
                Poll::Ready(Err(error.take().expect("Polled after completion").into()))
            }
        }
    }
}
