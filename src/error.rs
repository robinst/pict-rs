use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use color_eyre::Report;

pub(crate) struct Error {
    inner: color_eyre::Report,
}

impl Error {
    fn kind(&self) -> Option<&UploadError> {
        self.inner.downcast_ref()
    }

    pub(crate) fn root_cause(&self) -> &(dyn std::error::Error + 'static) {
        self.inner.root_cause()
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.inner, f)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.inner, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source()
    }
}

impl<T> From<T> for Error
where
    UploadError: From<T>,
{
    fn from(error: T) -> Self {
        Error {
            inner: Report::from(UploadError::from(error)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UploadError {
    #[error("Couldn't upload file")]
    Upload(#[from] actix_form_data::Error),

    #[error("Error in DB")]
    Repo(#[from] crate::repo::RepoError),

    #[error("Error in old repo")]
    OldRepo(#[from] crate::repo_04::RepoError),

    #[error("Error parsing string")]
    ParseString(#[from] std::string::FromUtf8Error),

    #[error("Error interacting with filesystem")]
    Io(#[from] std::io::Error),

    #[error("Error validating upload")]
    Validation(#[from] crate::validate::ValidationError),

    #[error("Error generating path")]
    PathGenerator(#[from] storage_path_generator::PathError),

    #[error("Error stripping prefix")]
    StripPrefix(#[from] std::path::StripPrefixError),

    #[error("Error in store")]
    Store(#[source] crate::store::StoreError),

    #[error("Error in ffmpeg")]
    Ffmpeg(#[from] crate::ffmpeg::FfMpegError),

    #[error("Error in imagemagick")]
    Magick(#[from] crate::magick::MagickError),

    #[error("Error in exiftool")]
    Exiftool(#[from] crate::exiftool::ExifError),

    #[error("Error building reqwest client")]
    BuildClient(#[source] reqwest::Error),

    #[error("Error making request")]
    RequestMiddleware(#[from] reqwest_middleware::Error),

    #[error("Error in request response")]
    Request(#[from] reqwest::Error),

    #[error("pict-rs is in read-only mode")]
    ReadOnly,

    #[error("Requested file extension cannot be served by source file")]
    InvalidProcessExtension,

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

    #[error("Requested a file that pict-rs lost track of")]
    MissingIdentifier,

    #[error("Provided token did not match expected token")]
    InvalidToken,

    #[error("Process endpoint was called with invalid extension")]
    UnsupportedProcessExtension,

    #[error("Unable to download image, bad response {0}")]
    Download(actix_web::http::StatusCode),

    #[error("Tried to save an image with an already-taken name")]
    DuplicateAlias,

    #[error("Error in json")]
    Json(#[from] serde_json::Error),

    #[error("Error in cbor")]
    Cbor(#[from] serde_cbor::Error),

    #[error("Range header not satisfiable")]
    Range,

    #[error("Hit limit")]
    Limit(#[from] crate::stream::LimitError),

    #[error("Response timeout")]
    Timeout(#[from] crate::stream::TimeoutError),
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

impl From<crate::store::StoreError> for UploadError {
    fn from(value: crate::store::StoreError) -> Self {
        match value {
            crate::store::StoreError::Repo(repo_error) => Self::Repo(repo_error),
            e => Self::Store(e),
        }
    }
}

impl ResponseError for Error {
    fn status_code(&self) -> StatusCode {
        match self.kind() {
            Some(
                UploadError::DuplicateAlias
                | UploadError::Limit(_)
                | UploadError::NoFiles
                | UploadError::Upload(_)
                | UploadError::Store(crate::store::StoreError::Repo(
                    crate::repo::RepoError::AlreadyClaimed,
                ))
                | UploadError::Repo(crate::repo::RepoError::AlreadyClaimed)
                | UploadError::Validation(_)
                | UploadError::UnsupportedProcessExtension
                | UploadError::InvalidProcessExtension
                | UploadError::ReadOnly,
            ) => StatusCode::BAD_REQUEST,
            Some(UploadError::Magick(e)) if e.is_client_error() => StatusCode::BAD_REQUEST,
            Some(UploadError::Ffmpeg(e)) if e.is_client_error() => StatusCode::BAD_REQUEST,
            Some(UploadError::Exiftool(e)) if e.is_client_error() => StatusCode::BAD_REQUEST,
            Some(UploadError::MissingAlias) => StatusCode::NOT_FOUND,
            Some(UploadError::Ffmpeg(e)) if e.is_not_found() => StatusCode::NOT_FOUND,
            Some(UploadError::InvalidToken) => StatusCode::FORBIDDEN,
            Some(UploadError::Range) => StatusCode::RANGE_NOT_SATISFIABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status_code())
            .content_type("application/json")
            .body(
                serde_json::to_string(&serde_json::json!({ "msg": self.root_cause().to_string() }))
                    .unwrap_or_else(|_| r#"{"msg":"Request failed"}"#.to_string()),
            )
    }
}
