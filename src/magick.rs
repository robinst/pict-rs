use std::sync::Arc;

use crate::{
    error_code::ErrorCode,
    formats::ProcessableFormat,
    process::{Process, ProcessError},
    store::Store,
};
use tokio::io::AsyncRead;

#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("Error in imagemagick process")]
    Process(#[source] ProcessError),

    #[error("Error in store")]
    Store(#[source] crate::store::StoreError),

    #[error("Invalid output format")]
    Json(#[source] serde_json::Error),

    #[error("Error reading bytes")]
    Read(#[source] std::io::Error),

    #[error("Error writing bytes")]
    Write(#[source] std::io::Error),

    #[error("Error creating file")]
    CreateFile(#[source] std::io::Error),

    #[error("Error creating directory")]
    CreateDir(#[source] crate::store::file_store::FileError),

    #[error("Error closing file")]
    CloseFile(#[source] std::io::Error),

    #[error("Error removing file")]
    RemoveFile(#[source] std::io::Error),

    #[error("Error in metadata discovery")]
    Discover(#[source] crate::discover::DiscoverError),

    #[error("Invalid media file provided")]
    CommandFailed(ProcessError),

    #[error("Command output is empty")]
    Empty,

    #[error("Invalid file path")]
    Path,
}

impl From<ProcessError> for MagickError {
    fn from(value: ProcessError) -> Self {
        match value {
            e @ ProcessError::Status(_, _) => Self::CommandFailed(e),
            otherwise => Self::Process(otherwise),
        }
    }
}

impl MagickError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::CommandFailed(_) => ErrorCode::COMMAND_FAILURE,
            Self::Store(e) => e.error_code(),
            Self::Process(e) => e.error_code(),
            Self::Json(_)
            | Self::Read(_)
            | Self::Write(_)
            | Self::CreateFile(_)
            | Self::CreateDir(_)
            | Self::CloseFile(_)
            | Self::RemoveFile(_)
            | Self::Discover(_)
            | Self::Empty
            | Self::Path => ErrorCode::COMMAND_ERROR,
        }
    }

    pub(crate) fn is_client_error(&self) -> bool {
        // Failing validation or imagemagick bailing probably means bad input
        matches!(
            self,
            Self::CommandFailed(_) | Self::Process(ProcessError::Timeout(_))
        )
    }
}

async fn process_image<F, Fut>(
    process_args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
    write_file: F,
) -> Result<impl AsyncRead + Unpin, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(MagickError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (write_file)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let input_arg = format!("{}:{input_file_str}", input_format.magick_format());
    let output_arg = format!("{}:-", format.magick_format());
    let quality = quality.map(|q| q.to_string());

    let len = if format.coalesce() {
        process_args.len() + 4
    } else {
        process_args.len() + 3
    };

    let mut args: Vec<&str> = Vec::with_capacity(len);
    args.push("convert");
    args.push(&input_arg);
    if format.coalesce() {
        args.push("-coalesce");
    }
    args.extend(process_args.iter().map(|s| s.as_str()));
    if let Some(quality) = &quality {
        args.extend(["-quality", quality]);
    }
    args.push(&output_arg);

    let reader = Process::run("magick", &args, timeout)?.read();

    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, input_file);

    Ok(Box::pin(clean_reader))
}

pub(crate) async fn process_image_store_read<S: Store + 'static>(
    store: &S,
    identifier: &Arc<str>,
    args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    let stream = store
        .to_stream(identifier, None, None)
        .await
        .map_err(MagickError::Store)?;

    process_image(
        args,
        input_format,
        format,
        quality,
        timeout,
        |mut tmp_file| async move {
            tmp_file
                .write_from_stream(stream)
                .await
                .map_err(MagickError::Write)?;
            Ok(tmp_file)
        },
    )
    .await
}

pub(crate) async fn process_image_async_read<A: AsyncRead + Unpin + 'static>(
    async_read: A,
    args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    process_image(
        args,
        input_format,
        format,
        quality,
        timeout,
        |mut tmp_file| async move {
            tmp_file
                .write_from_async_read(async_read)
                .await
                .map_err(MagickError::Write)?;
            Ok(tmp_file)
        },
    )
    .await
}
