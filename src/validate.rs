mod exiftool;
mod ffmpeg;
mod magick;

use crate::{
    either::Either,
    error::Error,
    formats::{AnimationOutput, ImageOutput, InputFile, InternalFormat, PrescribedFormats},
};
use actix_web::web::Bytes;
use tokio::io::AsyncRead;

#[tracing::instrument(skip_all)]
pub(crate) async fn validate_bytes(
    bytes: Bytes,
    prescribed: &PrescribedFormats,
) -> Result<(InternalFormat, impl AsyncRead + Unpin), Error> {
    let discovery = crate::discover::discover_bytes(bytes.clone()).await?;

    match &discovery.input {
        InputFile::Image(input) => {
            let ImageOutput {
                format,
                needs_transcode,
            } = input.build_output(prescribed.image);

            let read = if needs_transcode {
                Either::left(Either::left(magick::convert_image(
                    input.format,
                    format,
                    bytes,
                )?))
            } else {
                Either::left(Either::right(exiftool::clear_metadata_bytes_read(bytes)?))
            };

            Ok((InternalFormat::Image(format), read))
        }
        InputFile::Animation(input) => {
            let AnimationOutput {
                format,
                needs_transcode,
            } = input.build_output(prescribed.animation);

            let read = if needs_transcode {
                Either::right(Either::left(magick::convert_animation(
                    input.format,
                    format,
                    bytes,
                )?))
            } else {
                Either::right(Either::right(Either::left(
                    exiftool::clear_metadata_bytes_read(bytes)?,
                )))
            };

            Ok((InternalFormat::Animation(format), read))
        }
        InputFile::Video(input) => {
            let output = input.build_output(prescribed.video, prescribed.allow_audio);
            let read = Either::right(Either::right(Either::right(
                ffmpeg::transcode_bytes(*input, output, bytes).await?,
            )));

            Ok((InternalFormat::Video(output.internal_format()), read))
        }
    }
}
