use crate::process::Process;
use actix_web::web::Bytes;
use tokio::io::AsyncRead;

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::run("exiftool", &["-all=", "-", "-out", "-"])?;

    Ok(process.bytes_read(input))
}
