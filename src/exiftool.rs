use crate::{
    error_code::ErrorCode,
    process::{Process, ProcessError},
};
use actix_web::web::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExifError {
    #[error("Error in process")]
    Process(#[source] ProcessError),

    #[error("Error reading process output")]
    Read(#[source] std::io::Error),

    #[error("Invalid media file provided")]
    CommandFailed(ProcessError),
}

impl From<ProcessError> for ExifError {
    fn from(value: ProcessError) -> Self {
        match value {
            e @ ProcessError::Status(_, _) => Self::CommandFailed(e),
            otherwise => Self::Process(otherwise),
        }
    }
}

impl ExifError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Process(e) => e.error_code(),
            Self::Read(_) => ErrorCode::COMMAND_ERROR,
            Self::CommandFailed(_) => ErrorCode::COMMAND_FAILURE,
        }
    }
    pub(crate) fn is_client_error(&self) -> bool {
        // if exiftool bails we probably have bad input
        matches!(
            self,
            Self::CommandFailed(_) | Self::Process(ProcessError::Timeout(_))
        )
    }
}

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) async fn needs_reorienting(timeout: u64, input: Bytes) -> Result<bool, ExifError> {
    let process = Process::run("exiftool", &["-n", "-Orientation", "-"], &[], timeout)?;
    let mut reader = process.bytes_read(input);

    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .await
        .map_err(ExifError::Read)?;

    Ok(!buf.is_empty())
}

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) fn clear_metadata_bytes_read(
    timeout: u64,
    input: Bytes,
) -> Result<impl AsyncRead + Unpin, ExifError> {
    let process = Process::run("exiftool", &["-all=", "-", "-out", "-"], &[], timeout)?;

    Ok(process.bytes_read(input))
}
