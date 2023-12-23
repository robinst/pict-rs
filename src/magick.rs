use std::{ffi::OsStr, sync::Arc};

use crate::{
    error_code::ErrorCode,
    formats::ProcessableFormat,
    process::{Process, ProcessError, ProcessRead},
    read::BoxRead,
    store::Store,
    tmp_file::TmpDir,
};

use tokio::io::AsyncRead;

pub(crate) const MAGICK_TEMPORARY_PATH: &str = "MAGICK_TEMPORARY_PATH";

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

    #[error("Error creating temporary directory")]
    CreateTemporaryDirectory(#[source] std::io::Error),

    #[error("Error closing file")]
    CloseFile(#[source] std::io::Error),

    #[error("Error in metadata discovery")]
    Discover(#[source] crate::discover::DiscoverError),

    #[error("Invalid media file provided")]
    CommandFailed(ProcessError),

    #[error("Command output is empty")]
    Empty,
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
            | Self::CreateTemporaryDirectory(_)
            | Self::CloseFile(_)
            | Self::Discover(_)
            | Self::Empty => ErrorCode::COMMAND_ERROR,
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
    tmp_dir: &TmpDir,
    process_args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
    write_file: F,
) -> Result<ProcessRead, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let temporary_path = tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_file = tmp_dir.tmp_file(None);
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (write_file)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let input_arg = [
        input_format.magick_format().as_ref(),
        input_file.as_os_str(),
    ]
    .join(":".as_ref());
    let output_arg = format!("{}:-", format.magick_format());
    let quality = quality.map(|q| q.to_string());

    let len = 3
        + if input_format.coalesce() { 1 } else { 0 }
        + if quality.is_some() { 1 } else { 0 }
        + process_args.len();

    let mut args: Vec<&OsStr> = Vec::with_capacity(len);
    args.push("convert".as_ref());
    args.push(&input_arg);
    if input_format.coalesce() {
        args.push("-coalesce".as_ref());
    }
    args.extend(process_args.iter().map(AsRef::<OsStr>::as_ref));
    if let Some(quality) = &quality {
        args.extend(["-quality".as_ref(), quality.as_ref()] as [&OsStr; 2]);
    }
    args.push(output_arg.as_ref());

    let envs = [(MAGICK_TEMPORARY_PATH, temporary_path.as_os_str())];

    let reader = Process::run("magick", &args, &envs, timeout)?
        .read()
        .add_extras(input_file)
        .add_extras(temporary_path);

    Ok(reader)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_image_store_read<S: Store + 'static>(
    tmp_dir: &TmpDir,
    store: &S,
    identifier: &Arc<str>,
    args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
) -> Result<ProcessRead, MagickError> {
    let stream = store
        .to_stream(identifier, None, None)
        .await
        .map_err(MagickError::Store)?;

    process_image(
        tmp_dir,
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
    tmp_dir: &TmpDir,
    async_read: A,
    args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
) -> Result<ProcessRead, MagickError> {
    process_image(
        tmp_dir,
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

pub(crate) async fn process_image_process_read(
    tmp_dir: &TmpDir,
    process_read: ProcessRead,
    args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
) -> Result<ProcessRead, MagickError> {
    process_image(
        tmp_dir,
        args,
        input_format,
        format,
        quality,
        timeout,
        |mut tmp_file| async move {
            process_read
                .with_stdout(|stdout| async {
                    tmp_file
                        .write_from_async_read(stdout)
                        .await
                        .map_err(MagickError::Write)
                })
                .await??;

            Ok(tmp_file)
        },
    )
    .await
}
