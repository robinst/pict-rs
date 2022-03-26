use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use tracing_error::SpanTrace;

pub(crate) struct Error {
    context: SpanTrace,
    kind: UploadError,
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.kind)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.kind)?;
        std::fmt::Display::fmt(&self.context, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.kind.source()
    }
}

impl<T> From<T> for Error
where
    UploadError: From<T>,
{
    fn from(error: T) -> Self {
        Error {
            kind: UploadError::from(error),
            context: SpanTrace::capture(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UploadError {
    #[error("Couln't upload file")]
    Upload(#[from] actix_form_data::Error),

    #[error("Error in DB")]
    Sled(#[from] crate::repo::sled::Error),

    #[error("Error in old sled DB")]
    OldSled(#[from] ::sled::Error),

    #[error("Error parsing string")]
    ParseString(#[from] std::string::FromUtf8Error),

    #[error("Error interacting with filesystem")]
    Io(#[from] std::io::Error),

    #[error("Error generating path")]
    PathGenerator(#[from] storage_path_generator::PathError),

    #[error("Error stripping prefix")]
    StripPrefix(#[from] std::path::StripPrefixError),

    #[error("Error storing file")]
    FileStore(#[from] crate::store::file_store::FileError),

    #[error("Error storing object")]
    ObjectStore(#[from] crate::store::object_store::ObjectError),

    #[error("Provided process path is invalid")]
    ParsePath,

    #[error("Failed to acquire the semaphore")]
    Semaphore,

    #[error("Panic in blocking operation")]
    Canceled,

    #[error("No files present in upload")]
    NoFiles,

    #[error("Requested a file that doesn't exist")]
    MissingAlias,

    #[error("Provided token did not match expected token")]
    InvalidToken,

    #[error("Unsupported image format")]
    UnsupportedFormat,

    #[error("Invalid media dimensions")]
    Dimensions,

    #[error("Unable to download image, bad response {0}")]
    Download(actix_web::http::StatusCode),

    #[error("Unable to download image")]
    Payload(#[from] awc::error::PayloadError),

    #[error("Unable to send request, {0}")]
    SendRequest(String),

    #[error("No filename provided in request")]
    MissingFilename,

    #[error("Error converting Path to String")]
    Path,

    #[error("Tried to save an image with an already-taken name")]
    DuplicateAlias,

    #[error("Error in json")]
    Json(#[from] serde_json::Error),

    #[error("Range header not satisfiable")]
    Range,

    #[error("Hit limit")]
    Limit(#[from] super::LimitError),
}

impl From<awc::error::SendRequestError> for UploadError {
    fn from(e: awc::error::SendRequestError) -> Self {
        UploadError::SendRequest(e.to_string())
    }
}

impl From<actix_web::error::BlockingError> for UploadError {
    fn from(_: actix_web::error::BlockingError) -> Self {
        UploadError::Canceled
    }
}

impl From<tokio::sync::AcquireError> for UploadError {
    fn from(_: tokio::sync::AcquireError) -> Self {
        UploadError::Semaphore
    }
}

impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        match self.kind {
            UploadError::DuplicateAlias
            | UploadError::Limit(_)
            | UploadError::NoFiles
            | UploadError::Upload(_) => StatusCode::BAD_REQUEST,
            UploadError::MissingAlias | UploadError::MissingFilename => StatusCode::NOT_FOUND,
            UploadError::InvalidToken => StatusCode::FORBIDDEN,
            UploadError::Range => StatusCode::RANGE_NOT_SATISFIABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code())
            .content_type("application/json")
            .body(
                serde_json::to_string(&serde_json::json!({ "msg": self.kind.to_string() }))
                    .unwrap_or_else(|_| r#"{"msg":"Request failed"}"#.to_string()),
            )
    }
}
