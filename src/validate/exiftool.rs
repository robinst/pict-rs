use actix_web::web::Bytes;

use crate::{exiftool::ExifError, process::Process, read::BoxRead};

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) fn clear_metadata_bytes_read(
    input: Bytes,
    timeout: u64,
) -> Result<BoxRead<'static>, ExifError> {
    let process = Process::run("exiftool", &["-all=", "-", "-out", "-"], &[], timeout)?;

    Ok(Box::pin(process.bytes_read(input)))
}
