use crate::{
    bytes_stream::BytesStream,
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
    bytes: BytesStream,
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
async fn needs_reorienting(input: BytesStream, timeout: u64) -> Result<bool, ExifError> {
    let buf = Process::run("exiftool", &["-n", "-Orientation", "-"], &[], timeout)
        .await?
        .drive_with_async_read(input.into_reader())
        .into_string()
        .await?;

    Ok(!buf.is_empty())
}
