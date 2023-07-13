use crate::{
    formats::ProcessableFormat,
    process::{Process, ProcessError},
    store::Store,
};
use tokio::io::AsyncRead;

#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("Error in imagemagick process")]
    Process(#[source] ProcessError),

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

    #[error("Invalid file path")]
    Path,
}

impl MagickError {
    pub(crate) fn is_client_error(&self) -> bool {
        // Failing validation or imagemagick bailing probably means bad input
        matches!(self, Self::Process(ProcessError::Status(_)))
    }
}

fn process_image(
    process_args: Vec<String>,
    format: ProcessableFormat,
) -> Result<Process, ProcessError> {
    let command = "magick";
    let convert_args = ["convert", "-"];
    let last_arg = format!("{}:-", format.magick_format());

    let len = if format.coalesce() {
        process_args.len() + 4
    } else {
        process_args.len() + 3
    };

    let mut args = Vec::with_capacity(len);
    args.extend_from_slice(&convert_args[..]);
    args.extend(process_args.iter().map(|s| s.as_str()));
    if format.coalesce() {
        args.push("-coalesce");
    }
    args.push(&last_arg);

    Process::run(command, &args)
}

pub(crate) fn process_image_store_read<S: Store + 'static>(
    store: S,
    identifier: S::Identifier,
    args: Vec<String>,
    format: ProcessableFormat,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    Ok(process_image(args, format)
        .map_err(MagickError::Process)?
        .store_read(store, identifier))
}

pub(crate) fn process_image_async_read<A: AsyncRead + Unpin + 'static>(
    async_read: A,
    args: Vec<String>,
    format: ProcessableFormat,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    Ok(process_image(args, format)
        .map_err(MagickError::Process)?
        .pipe_async_read(async_read))
}
