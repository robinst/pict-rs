use actix_web::{
    dev::{Service, ServiceRequest, Transform},
    http::StatusCode,
    HttpResponse, ResponseError,
};
use std::{
    future::{ready, Future, Ready},
    pin::Pin,
    task::{Context, Poll},
};

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
        HttpResponse::build(self.status_code()).json(serde_json::json!({
            "msg": self.to_string(),
            "code": "invalid-api-token",
        }))
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
            InternalFutureProj::Error { error } => {
                Poll::Ready(Err(error.take().expect("Polled after completion").into()))
            }
        }
    }
}
