use actix_web::web::Bytes;

use crate::{
    exiftool::ExifError,
    process::{Process, ProcessRead},
    read::BoxRead,
};

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) fn clear_metadata_bytes_read(
    input: Bytes,
    timeout: u64,
) -> Result<ProcessRead, ExifError> {
    Ok(Process::run("exiftool", &["-all=", "-", "-out", "-"], &[], timeout)?.bytes_read(input))
}
