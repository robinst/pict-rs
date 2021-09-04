use crate::stream::Process;
use actix_web::web::Bytes;
use tokio::{io::AsyncRead, process::Command};

pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::spawn(Command::new("exiftool").args(["-all=", "-", "-out", "-"]))?;

    Ok(process.bytes_read(input).unwrap())
}
