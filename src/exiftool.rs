use crate::{error_code::ErrorCode, process::ProcessError};

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
