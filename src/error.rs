use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use tracing_error::SpanTrace;

pub(crate) struct Error {
    context: SpanTrace,
    kind: UploadError,
}

impl Error {
    pub(crate) fn kind(&self) -> &UploadError {
        &self.kind
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n", self.kind)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n", self.kind)?;
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

impl From<sled::transaction::TransactionError<Error>> for Error {
    fn from(e: sled::transaction::TransactionError<Error>) -> Self {
        match e {
            sled::transaction::TransactionError::Abort(t) => t,
            sled::transaction::TransactionError::Storage(e) => e.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UploadError {
    #[error("Couln't upload file, {0}")]
    Upload(#[from] actix_form_data::Error),

    #[error("Error in DB, {0}")]
    Db(#[from] sled::Error),

    #[error("Error parsing string, {0}")]
    ParseString(#[from] std::string::FromUtf8Error),

    #[error("Error parsing request, {0}")]
    ParseReq(String),

    #[error("Error interacting with filesystem, {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    PathGenerator(#[from] storage_path_generator::PathError),

    #[error(transparent)]
    StripPrefix(#[from] std::path::StripPrefixError),

    #[error("Failed to acquire the semaphore")]
    Semaphore,

    #[error("Panic in blocking operation")]
    Canceled,

    #[error("No files present in upload")]
    NoFiles,

    #[error("Requested a file that doesn't exist")]
    MissingAlias,

    #[error("Alias directed to missing file")]
    MissingFile,

    #[error("Provided token did not match expected token")]
    InvalidToken,

    #[error("Unsupported image format")]
    UnsupportedFormat,

    #[error("Invalid media dimensions")]
    Dimensions,

    #[error("Unable to download image, bad response {0}")]
    Download(actix_web::http::StatusCode),

    #[error("Unable to download image, {0}")]
    Payload(#[from] awc::error::PayloadError),

    #[error("Unable to send request, {0}")]
    SendRequest(String),

    #[error("No filename provided in request")]
    MissingFilename,

    #[error("Error converting Path to String")]
    Path,

    #[error("Tried to save an image with an already-taken name")]
    DuplicateAlias,

    #[error("Tried to create file, but file already exists")]
    FileExists,

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    #[error("Range header not satisfiable")]
    Range,

    #[error("Command failed")]
    Status,

    #[error(transparent)]
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
            | UploadError::Upload(_)
            | UploadError::ParseReq(_) => StatusCode::BAD_REQUEST,
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
