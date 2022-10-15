use crate::process::Process;
use actix_web::web::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) async fn needs_reorienting(input: Bytes) -> std::io::Result<bool> {
    let process = Process::run("exiftool", &["-n", "-Orientation", "-"])?;
    let mut reader = process.bytes_read(input);

    let mut buf = String::new();
    reader.read_to_string(&mut buf).await?;

    Ok(!buf.is_empty())
}

#[tracing::instrument(level = "trace", skip(input))]
pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::run("exiftool", &["-all=", "-", "-out", "-"])?;

    Ok(process.bytes_read(input))
}
