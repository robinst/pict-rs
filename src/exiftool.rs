use crate::{
    bytes_stream::BytesStream,
    error_code::ErrorCode,
    process::{Process, ProcessError, ProcessRead},
};


#[derive(Debug, thiserror::Error)]
pub(crate) enum ExifError {
    #[error("Error in process")]
    Process(#[source] ProcessError),

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
            Self::CommandFailed(_) => ErrorCode::COMMAND_FAILURE,
        }
    }
    pub(crate) fn is_client_error(&self) -> bool {
        // if exiftool bails we probably have bad input
        match self {
            Self::CommandFailed(_) => true,
            Self::Process(e) => e.is_client_error(),
        }
    }
}

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) async fn needs_reorienting(timeout: u64, input: BytesStream) -> Result<bool, ExifError> {
    let buf = Process::run("exiftool", &["-n", "-Orientation", "-"], &[], timeout)?
        .bytes_stream_read(input)
        .into_string()
        .await?;

    Ok(!buf.is_empty())
}

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) fn clear_metadata_bytes_read(
    timeout: u64,
    input: BytesStream,
) -> Result<ProcessRead, ExifError> {
    let process = Process::run("exiftool", &["-all=", "-", "-out", "-"], &[], timeout)?;

    Ok(process.bytes_stream_read(input))
}
