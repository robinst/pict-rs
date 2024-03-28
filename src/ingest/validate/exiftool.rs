use crate::{exiftool::ExifError, process::Process};

#[tracing::instrument(level = "trace", skip_all)]
pub(super) async fn clear_metadata_command(timeout: u64) -> Result<Process, ExifError> {
    Ok(Process::run("exiftool", &["-all=", "-", "-out", "-"], &[], timeout).await?)
}
