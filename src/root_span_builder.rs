use actix_web::{
    dev::{ServiceRequest, ServiceResponse},
    Error,
};
use tracing::Span;
use tracing_actix_web::root_span;

pub struct RootSpanBuilder;

impl tracing_actix_web::RootSpanBuilder for RootSpanBuilder {
    fn on_request_start(request: &ServiceRequest) -> Span {
        root_span!(request)
    }

    fn on_request_end<B>(span: Span, outcome: &Result<ServiceResponse<B>, Error>) {
        match &outcome {
            Ok(response) => {
                if let Some(error) = response.response().error() {
                    handle_error(span, error)
                } else {
                    span.record("http.status_code", &response.response().status().as_u16());
                    span.record("otel.status_code", &"OK");
                }
            }
            Err(error) => handle_error(span, error),
        }
    }
}

fn handle_error(span: Span, error: &Error) {
    let response_error = error.as_response_error();

    let display = format!("{}", response_error);
    let debug = format!("{:?}", response_error);
    span.record("exception.message", &tracing::field::display(display));
    span.record("exception.details", &tracing::field::display(debug));

    let status_code = response_error.status_code();
    span.record("http.status_code", &status_code.as_u16());

    if status_code.is_client_error() {
        span.record("otel.status_code", &"OK");
    } else {
        span.record("otel.status_code", &"ERROR");
    }
}
