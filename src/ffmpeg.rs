use std::ffi::OsString;

use futures_core::Future;

use crate::{error_code::ErrorCode, process::ProcessError, store::StoreError, tmp_file::TmpDir};

#[derive(Debug, thiserror::Error)]
pub(crate) enum FfMpegError {
    #[error("Error in ffmpeg process")]
    Process(#[source] ProcessError),

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

    #[error("Error cleaning up after command")]
    Cleanup(#[source] std::io::Error),

    #[error("Error in store")]
    Store(#[source] StoreError),

    #[error("Invalid media file provided")]
    CommandFailed(ProcessError),
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
            Self::Write(_)
            | Self::Json(_)
            | Self::CreateDir(_)
            | Self::ReadFile(_)
            | Self::OpenFile(_)
            | Self::Cleanup(_) => ErrorCode::COMMAND_ERROR,
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

pub(crate) async fn with_file<F, Fut>(
    tmp: &TmpDir,
    ext: Option<&str>,
    f: F,
) -> Result<Fut::Output, FfMpegError>
where
    F: FnOnce(OsString) -> Fut,
    Fut: Future,
{
    let file = tmp.tmp_file(ext);

    crate::store::file_store::safe_create_parent(&file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let res = (f)(file.as_os_str().to_os_string()).await;

    file.cleanup().await.map_err(FfMpegError::Cleanup)?;

    Ok(res)
}
