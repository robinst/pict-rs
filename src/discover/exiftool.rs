use actix_web::web::Bytes;

use crate::{
    exiftool::ExifError,
    formats::{ImageInput, InputFile},
    process::Process,
};

use super::Discovery;

#[tracing::instrument(level = "debug", skip_all)]
pub(super) async fn check_reorient(
    Discovery {
        input,
        width,
        height,
        frames,
    }: Discovery,
    bytes: Bytes,
    timeout: u64,
) -> Result<Discovery, ExifError> {
    let input = match input {
        InputFile::Image(ImageInput { format, .. }) => {
            let needs_reorient = needs_reorienting(bytes, timeout).await?;

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

#[tracing::instrument(level = "trace", skip_all)]
async fn needs_reorienting(input: Bytes, timeout: u64) -> Result<bool, ExifError> {
    let buf = Process::run("exiftool", &["-n", "-Orientation", "-"], &[], timeout)?
        .bytes_read(input)
        .into_string()
        .await?;

    Ok(!buf.is_empty())
}
