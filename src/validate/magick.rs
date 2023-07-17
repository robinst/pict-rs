use actix_web::web::Bytes;
use tokio::io::AsyncRead;

use crate::{
    formats::{AnimationFormat, ImageFormat, OutputVideoFormat},
    magick::MagickError,
    process::Process,
};

pub(super) async fn convert_image(
    input: ImageFormat,
    output: ImageFormat,
    bytes: Bytes,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    convert(input.magick_format(), output.magick_format(), false, bytes).await
}

pub(super) async fn convert_animation(
    input: AnimationFormat,
    output: AnimationFormat,
    bytes: Bytes,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    convert(input.magick_format(), output.magick_format(), true, bytes).await
}

pub(super) async fn convert_video(
    input: AnimationFormat,
    output: OutputVideoFormat,
    bytes: Bytes,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    convert(input.magick_format(), output.magick_format(), true, bytes).await
}

async fn convert(
    input: &'static str,
    output: &'static str,
    coalesce: bool,
    bytes: Bytes,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(MagickError::Path)?;

    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let mut tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    tmp_one
        .write_from_bytes(bytes)
        .await
        .map_err(MagickError::Write)?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let input_arg = format!("{input}:{input_file_str}");
    let output_arg = format!("{output}:-");

    let process = if coalesce {
        Process::run(
            "magick",
            &[
                "convert",
                "-strip",
                "-auto-orient",
                &input_arg,
                "-coalesce",
                &output_arg,
            ],
        )?
    } else {
        Process::run(
            "magick",
            &["convert", "-strip", "-auto-orient", &input_arg, &output_arg],
        )?
    };

    let reader = process.read();

    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, input_file);

    Ok(Box::pin(clean_reader))
}
