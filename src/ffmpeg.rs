use crate::{error_code::ErrorCode, process::ProcessError, store::StoreError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum FfMpegError {
    #[error("Error in ffmpeg process")]
    Process(#[source] ProcessError),

    #[error("Error reading output")]
    Read(#[source] std::io::Error),

    #[error("Error writing bytes")]
    Write(#[source] std::io::Error),

    #[error("Invalid output format")]
    Json(#[source] serde_json::Error),

    #[error("Error creating parent directory")]
    CreateDir(#[source] crate::store::file_store::FileError),

    #[error("Error reading file to stream")]
    ReadFile(#[source] crate::store::file_store::FileError),

    #[error("Error opening file")]
    OpenFile(#[source] std::io::Error),

    #[error("Error creating file")]
    CreateFile(#[source] std::io::Error),

    #[error("Error closing file")]
    CloseFile(#[source] std::io::Error),

    #[error("Error removing file")]
    RemoveFile(#[source] std::io::Error),

    #[error("Error in store")]
    Store(#[source] StoreError),

    #[error("Invalid media file provided")]
    CommandFailed(ProcessError),

    #[error("Invalid file path")]
    Path,
}

impl From<ProcessError> for FfMpegError {
    fn from(value: ProcessError) -> Self {
        match value {
            e @ ProcessError::Status(_, _) => Self::CommandFailed(e),
            otherwise => Self::Process(otherwise),
        }
    }
}

impl FfMpegError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::CommandFailed(_) => ErrorCode::COMMAND_FAILURE,
            Self::Store(s) => s.error_code(),
            Self::Process(e) => e.error_code(),
            Self::Read(_)
            | Self::Write(_)
            | Self::Json(_)
            | Self::CreateDir(_)
            | Self::ReadFile(_)
            | Self::OpenFile(_)
            | Self::CreateFile(_)
            | Self::CloseFile(_)
            | Self::RemoveFile(_)
            | Self::Path => ErrorCode::COMMAND_ERROR,
        }
    }

    pub(crate) fn is_client_error(&self) -> bool {
        // Failing validation or ffmpeg bailing probably means bad input
        matches!(
            self,
            Self::CommandFailed(_) | Self::Process(ProcessError::Timeout(_))
        )
    }

    pub(crate) fn is_not_found(&self) -> bool {
        if let Self::Store(e) = self {
            return e.is_not_found();
        }

        false
    }
}
