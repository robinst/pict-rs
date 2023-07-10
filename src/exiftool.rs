use crate::process::{Process, ProcessError};
use actix_web::web::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExifError {
    #[error("Error in process")]
    Process(#[source] ProcessError),

    #[error("Error reading process output")]
    Read(#[source] std::io::Error),
}

impl ExifError {
    pub(crate) fn is_client_error(&self) -> bool {
        // if exiftool bails we probably have bad input
        matches!(self, Self::Process(ProcessError::Status(_)))
    }
}

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) async fn needs_reorienting(input: Bytes) -> Result<bool, ExifError> {
    let process =
        Process::run("exiftool", &["-n", "-Orientation", "-"]).map_err(ExifError::Process)?;
    let mut reader = process.bytes_read(input);

    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .await
        .map_err(ExifError::Read)?;

    Ok(!buf.is_empty())
}

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> Result<impl AsyncRead + Unpin, ExifError> {
    let process =
        Process::run("exiftool", &["-all=", "-", "-out", "-"]).map_err(ExifError::Process)?;

    Ok(process.bytes_read(input))
}
