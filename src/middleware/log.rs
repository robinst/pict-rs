use std::future::{ready, Future, Ready};

use actix_web::{
    body::MessageBody,
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    http::StatusCode,
    ResponseError,
};

pub(crate) struct Log {
    info: bool,
}
pub(crate) struct LogMiddleware<S> {
    info: bool,
    inner: S,
}

impl Log {
    pub(crate) fn new(info: bool) -> Self {
        Self { info }
    }
}

#[derive(Debug)]
pub(crate) struct LogError {
    info: bool,
    error: actix_web::Error,
}

pin_project_lite::pin_project! {
    pub(crate) struct LogFuture<F> {
        info: bool,

        #[pin]
        inner: F,
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct LogBody<B> {
        info: bool,

        status: Option<StatusCode>,

        #[pin]
        inner: B,
    }
}

impl<S, B> Transform<S, ServiceRequest> for Log
where
    B: MessageBody,
    S: Service<ServiceRequest, Response = ServiceResponse<B>>,
    S::Future: 'static,
    S::Error: Into<actix_web::Error>,
{
    type Response = ServiceResponse<LogBody<B>>;
    type Error = actix_web::Error;
    type InitError = ();
    type Transform = LogMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(LogMiddleware {
            info: self.info,
            inner: service,
        }))
    }
}

impl<S, B> Service<ServiceRequest> for LogMiddleware<S>
where
    B: MessageBody,
    S: Service<ServiceRequest, Response = ServiceResponse<B>>,
    S::Future: 'static,
    S::Error: Into<actix_web::Error>,
{
    type Response = ServiceResponse<LogBody<B>>;
    type Error = actix_web::Error;
    type Future = LogFuture<S::Future>;

    fn poll_ready(
        &self,
        ctx: &mut core::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(ctx).map(|res| {
            res.map_err(|e| {
                LogError {
                    info: self.info,
                    error: e.into(),
                }
                .into()
            })
        })
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        LogFuture {
            info: self.info,
            inner: self.inner.call(req),
        }
    }
}

impl<F, B, E> Future for LogFuture<F>
where
    B: MessageBody,
    F: Future<Output = Result<ServiceResponse<B>, E>>,
    E: Into<actix_web::Error>,
{
    type Output = Result<ServiceResponse<LogBody<B>>, actix_web::Error>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let info = self.info;
        let this = self.project();

        std::task::Poll::Ready(match std::task::ready!(this.inner.poll(cx)) {
            Ok(response) => {
                let status = response.status();

                let status = if response.response().body().size().is_eof() {
                    emit(status, info);
                    None
                } else {
                    Some(status)
                };

                Ok(response.map_body(|_, inner| LogBody {
                    info,
                    status,
                    inner,
                }))
            }
            Err(e) => Err(LogError {
                info,
                error: e.into(),
            }
            .into()),
        })
    }
}

impl<B> MessageBody for LogBody<B>
where
    B: MessageBody,
{
    type Error = B::Error;

    fn size(&self) -> actix_web::body::BodySize {
        self.inner.size()
    }

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<actix_web::web::Bytes, Self::Error>>> {
        let this = self.project();

        let opt = std::task::ready!(this.inner.poll_next(cx));

        if opt.is_none() {
            if let Some(status) = this.status.take() {
                emit(status, *this.info);
            }
        }

        std::task::Poll::Ready(opt)
    }
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for LogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

impl ResponseError for LogError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        self.error.as_response_error().status_code()
    }

    fn error_response(&self) -> actix_web::HttpResponse<actix_web::body::BoxBody> {
        let response = self.error.error_response();
        let status = response.status();

        if response.body().size().is_eof() {
            emit(status, self.info);
            response
        } else {
            response.map_body(|_, inner| {
                LogBody {
                    info: self.info,
                    status: Some(status),
                    inner,
                }
                .boxed()
            })
        }
    }
}

fn emit(status: StatusCode, info: bool) {
    if status.is_server_error() {
        tracing::error!("server error");
    } else if status.is_client_error() {
        tracing::warn!("client error");
    } else if status.is_redirection() {
        tracing::info!("redirected");
    } else if info {
        tracing::info!("completed");
    } else {
        tracing::debug!("completed");
    }
}
