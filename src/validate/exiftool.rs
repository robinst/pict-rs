use crate::{
    bytes_stream::BytesStream,
    exiftool::ExifError,
    process::{Process, ProcessRead},
};

#[tracing::instrument(level = "trace", skip_all)]
pub(super) fn clear_metadata_bytes_read(
    input: BytesStream,
    timeout: u64,
) -> Result<ProcessRead, ExifError> {
    Ok(
        Process::run("exiftool", &["-all=", "-", "-out", "-"], &[], timeout)?
            .bytes_stream_read(input),
    )
}
