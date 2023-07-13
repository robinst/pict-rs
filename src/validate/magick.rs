use actix_web::web::Bytes;
use tokio::io::AsyncRead;

use crate::{
    formats::{AnimationFormat, ImageFormat},
    magick::MagickError,
    process::Process,
};

pub(super) fn convert_image(
    input: ImageFormat,
    output: ImageFormat,
    bytes: Bytes,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    let input_arg = format!("{}:-", input.magick_format());
    let output_arg = format!("{}:-", output.magick_format());

    let process = Process::run(
        "magick",
        &["-strip", "-auto-orient", &input_arg, &output_arg],
    )
    .map_err(MagickError::Process)?;

    Ok(process.bytes_read(bytes))
}

pub(super) fn convert_animation(
    input: AnimationFormat,
    output: AnimationFormat,
    bytes: Bytes,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    let input_arg = format!("{}:-", input.magick_format());
    let output_arg = format!("{}:-", output.magick_format());

    let process = Process::run("magick", &["-strip", &input_arg, "-coalesce", &output_arg])
        .map_err(MagickError::Process)?;

    Ok(process.bytes_read(bytes))
}
