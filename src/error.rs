use crate::validate::GifError;
use actix_web::{http::StatusCode, HttpResponse, ResponseError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum UploadError {
    #[error("Couln't upload file, {0}")]
    Upload(String),

    #[error("Error in DB, {0}")]
    Db(#[from] sled::Error),

    #[error("Error parsing string, {0}")]
    ParseString(#[from] std::string::FromUtf8Error),

    #[error("Error parsing request, {0}")]
    ParseReq(String),

    #[error("Error interacting with filesystem, {0}")]
    Io(#[from] std::io::Error),

    #[error("Error in filesyste, {0}")]
    Fs(#[from] actix_fs::Error),

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

    #[error("Unable to download image, bad response {0}")]
    Download(actix_web::http::StatusCode),

    #[error("Unable to download image, {0}")]
    Payload(#[from] actix_web::client::PayloadError),

    #[error("Unable to send request, {0}")]
    SendRequest(String),

    #[error("No filename provided in request")]
    MissingFilename,

    #[error("Error converting Path to String")]
    Path,

    #[error("Tried to save an image with an already-taken name")]
    DuplicateAlias,

    #[error("Error validating Gif file, {0}")]
    Gif(#[from] GifError),

    #[error("Error transcoding, {0}")]
    Transcode(crate::validate::transcode::Error),

    #[error("Tried to create file, but file already exists")]
    FileExists,

    #[error("File metadata could not be parsed, {0}")]
    Validate(#[from] rexiv2::Rexiv2Error),

    #[error("Error in MagickWand, {0}")]
    Wand(String),

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    #[error("Range header not satisfiable")]
    Range,
}

impl From<actix_web::client::SendRequestError> for UploadError {
    fn from(e: actix_web::client::SendRequestError) -> Self {
        UploadError::SendRequest(e.to_string())
    }
}

impl From<sled::transaction::TransactionError<UploadError>> for UploadError {
    fn from(e: sled::transaction::TransactionError<UploadError>) -> Self {
        match e {
            sled::transaction::TransactionError::Abort(t) => t,
            sled::transaction::TransactionError::Storage(e) => e.into(),
        }
    }
}

impl From<actix_form_data::Error> for UploadError {
    fn from(e: actix_form_data::Error) -> Self {
        UploadError::Upload(e.to_string())
    }
}

impl From<actix_web::error::BlockingError> for UploadError {
    fn from(_: actix_web::error::BlockingError) -> Self {
        UploadError::Canceled
    }
}

impl ResponseError for UploadError {
    fn status_code(&self) -> StatusCode {
        match self {
            UploadError::Gif(_)
            | UploadError::DuplicateAlias
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
            .json(&serde_json::json!({ "msg": self.to_string() }))
    }
}
