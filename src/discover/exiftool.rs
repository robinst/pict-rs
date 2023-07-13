use actix_web::web::Bytes;
use tokio::io::AsyncReadExt;

use crate::{
    exiftool::ExifError,
    formats::{ImageInput, InputFile},
    process::Process,
};

use super::Discovery;

pub(super) async fn check_reorient(
    Discovery {
        input,
        width,
        height,
        frames,
    }: Discovery,
    bytes: Bytes,
) -> Result<Discovery, ExifError> {
    let input = match input {
        InputFile::Image(ImageInput { format, .. }) => {
            let needs_reorient = needs_reorienting(bytes).await?;

            InputFile::Image(ImageInput {
                format,
                needs_reorient,
            })
        }
        otherwise => otherwise,
    };

    Ok(Discovery {
        input,
        width,
        height,
        frames,
    })
}

#[tracing::instrument(level = "trace", skip(input))]
async fn needs_reorienting(input: Bytes) -> Result<bool, ExifError> {
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
