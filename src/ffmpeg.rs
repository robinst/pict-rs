use std::sync::Arc;

use crate::{
    error_code::ErrorCode,
    formats::InternalVideoFormat,
    process::{Process, ProcessError},
    store::{Store, StoreError},
};
use tokio::io::AsyncRead;

#[derive(Clone, Copy, Debug)]
pub(crate) enum ThumbnailFormat {
    Jpeg,
    // Webp,
}

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

impl ThumbnailFormat {
    const fn as_ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Jpeg => "mjpeg",
            // Self::Webp => "webp",
        }
    }

    const fn to_file_extension(self) -> &'static str {
        match self {
            Self::Jpeg => ".jpeg",
            // Self::Webp => ".webp",
        }
    }

    const fn as_ffmpeg_format(self) -> &'static str {
        match self {
            Self::Jpeg => "image2",
            // Self::Webp => "webp",
        }
    }

    pub(crate) fn media_type(self) -> mime::Mime {
        match self {
            Self::Jpeg => mime::IMAGE_JPEG,
            // Self::Webp => crate::formats::mimes::image_webp(),
        }
    }
}

#[tracing::instrument(skip(store))]
pub(crate) async fn thumbnail<S: Store>(
    store: S,
    from: Arc<str>,
    input_format: InternalVideoFormat,
    format: ThumbnailFormat,
    timeout: u64,
) -> Result<impl AsyncRead + Unpin, FfMpegError> {
    let input_file = crate::tmp_file::tmp_file(Some(input_format.file_extension()));
    let input_file_str = input_file.to_str().ok_or(FfMpegError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let output_file = crate::tmp_file::tmp_file(Some(format.to_file_extension()));
    let output_file_str = output_file.to_str().ok_or(FfMpegError::Path)?;
    crate::store::file_store::safe_create_parent(&output_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let mut tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(FfMpegError::CreateFile)?;
    let stream = store
        .to_stream(&from, None, None)
        .await
        .map_err(FfMpegError::Store)?;
    tmp_one
        .write_from_stream(stream)
        .await
        .map_err(FfMpegError::Write)?;
    tmp_one.close().await.map_err(FfMpegError::CloseFile)?;

    let process = Process::run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "warning",
            "-i",
            input_file_str,
            "-frames:v",
            "1",
            "-codec",
            format.as_ffmpeg_codec(),
            "-f",
            format.as_ffmpeg_format(),
            output_file_str,
        ],
        timeout,
    )?;

    process.wait().await?;
    tokio::fs::remove_file(input_file)
        .await
        .map_err(FfMpegError::RemoveFile)?;

    let tmp_two = crate::file::File::open(&output_file)
        .await
        .map_err(FfMpegError::OpenFile)?;
    let stream = tmp_two
        .read_to_stream(None, None)
        .await
        .map_err(FfMpegError::ReadFile)?;
    let reader = tokio_util::io::StreamReader::new(stream);
    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, output_file);

    Ok(Box::pin(clean_reader))
}
