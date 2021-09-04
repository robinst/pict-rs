use crate::stream::Process;
use actix_web::web::Bytes;
use tokio::{io::AsyncRead, process::Command};

pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::spawn(Command::new("exiftool").args(["-all=", "-", "-out", "-"]))?;

    Ok(process.bytes_read(input).unwrap())
}

pub(crate) fn clear_metadata_write_read(
    input: impl AsyncRead + Unpin + 'static,
) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::spawn(Command::new("exiftool").args(["-all=", "-", "-out", "-"]))?;

    Ok(process.write_read(input).unwrap())
}

pub(crate) fn clear_metadata_stream<S, E>(
    input: S,
) -> std::io::Result<futures::stream::LocalBoxStream<'static, Result<actix_web::web::Bytes, E>>>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E>> + Unpin + 'static,
    E: From<std::io::Error> + 'static,
{
    let process = Process::spawn(Command::new("exiftool").args(["-all=", "-", "-out", "-"]))?;

    Ok(Box::pin(process.sink_stream(input).unwrap()))
}
