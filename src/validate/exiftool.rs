use actix_web::web::Bytes;
use tokio::io::AsyncRead;

use crate::{exiftool::ExifError, process::Process};

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> Result<impl AsyncRead + Unpin, ExifError> {
    let process = Process::run("exiftool", &["-all=", "-", "-out", "-"])?;

    Ok(process.bytes_read(input))
}
