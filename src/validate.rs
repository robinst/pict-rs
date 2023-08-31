mod exiftool;
mod ffmpeg;
mod magick;

use crate::{
    discover::Discovery,
    either::Either,
    error::Error,
    formats::{
        AnimationFormat, AnimationOutput, ImageInput, ImageOutput, InputFile, InputVideoFormat,
        InternalFormat, Validations,
    },
};
use actix_web::web::Bytes;
use tokio::io::AsyncRead;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidationError {
    #[error("Too wide")]
    Width,

    #[error("Too tall")]
    Height,

    #[error("Too many pixels")]
    Area,

    #[error("Too many frames")]
    Frames,

    #[error("Uploaded file is empty")]
    Empty,

    #[error("Filesize too large")]
    Filesize,

    #[error("Video is disabled")]
    VideoDisabled,
}

const MEGABYTES: usize = 1024 * 1024;

#[tracing::instrument(skip_all)]
pub(crate) async fn validate_bytes(
    bytes: Bytes,
    validations: Validations<'_>,
    timeout: u64,
) -> Result<(InternalFormat, impl AsyncRead + Unpin), Error> {
    if bytes.is_empty() {
        return Err(ValidationError::Empty.into());
    }

    let Discovery {
        input,
        width,
        height,
        frames,
    } = crate::discover::discover_bytes(timeout, bytes.clone()).await?;

    match &input {
        InputFile::Image(input) => {
            let (format, read) =
                process_image(bytes, *input, width, height, validations.image, timeout).await?;

            Ok((format, Either::left(read)))
        }
        InputFile::Animation(input) => {
            let (format, read) = process_animation(
                bytes,
                *input,
                width,
                height,
                frames.unwrap_or(1),
                validations.animation,
                timeout,
            )
            .await?;

            Ok((format, Either::right(Either::left(read))))
        }
        InputFile::Video(input) => {
            let (format, read) = process_video(
                bytes,
                input.clone(),
                width,
                height,
                frames.unwrap_or(1),
                validations.video,
                timeout,
            )
            .await?;

            Ok((format, Either::right(Either::right(read))))
        }
    }
}

#[tracing::instrument(skip(bytes, validations))]
async fn process_image(
    bytes: Bytes,
    input: ImageInput,
    width: u16,
    height: u16,
    validations: &crate::config::Image,
    timeout: u64,
) -> Result<(InternalFormat, impl AsyncRead + Unpin), Error> {
    if width > validations.max_width {
        return Err(ValidationError::Width.into());
    }
    if height > validations.max_height {
        return Err(ValidationError::Height.into());
    }
    if u32::from(width) * u32::from(height) > validations.max_area {
        return Err(ValidationError::Area.into());
    }
    if bytes.len() > validations.max_file_size * MEGABYTES {
        return Err(ValidationError::Filesize.into());
    }

    let ImageOutput {
        format,
        needs_transcode,
    } = input.build_output(validations.format);

    let read = if needs_transcode {
        let quality = validations.quality_for(format);

        Either::left(magick::convert_image(input.format, format, quality, timeout, bytes).await?)
    } else {
        Either::right(exiftool::clear_metadata_bytes_read(bytes, timeout)?)
    };

    Ok((InternalFormat::Image(format), read))
}

fn validate_animation(
    size: usize,
    width: u16,
    height: u16,
    frames: u32,
    validations: &crate::config::Animation,
) -> Result<(), ValidationError> {
    if width > validations.max_width {
        return Err(ValidationError::Width);
    }
    if height > validations.max_height {
        return Err(ValidationError::Height);
    }
    if u32::from(width) * u32::from(height) > validations.max_area {
        return Err(ValidationError::Area);
    }
    if frames > validations.max_frame_count {
        return Err(ValidationError::Frames);
    }
    if size > validations.max_file_size * MEGABYTES {
        return Err(ValidationError::Filesize);
    }

    Ok(())
}

#[tracing::instrument(skip(bytes, validations))]
async fn process_animation(
    bytes: Bytes,
    input: AnimationFormat,
    width: u16,
    height: u16,
    frames: u32,
    validations: &crate::config::Animation,
    timeout: u64,
) -> Result<(InternalFormat, impl AsyncRead + Unpin), Error> {
    validate_animation(bytes.len(), width, height, frames, validations)?;

    let AnimationOutput {
        format,
        needs_transcode,
    } = input.build_output(validations.format);

    let read = if needs_transcode {
        let quality = validations.quality_for(format);

        Either::left(magick::convert_animation(input, format, quality, timeout, bytes).await?)
    } else {
        Either::right(exiftool::clear_metadata_bytes_read(bytes, timeout)?)
    };

    Ok((InternalFormat::Animation(format), read))
}

fn validate_video(
    size: usize,
    width: u16,
    height: u16,
    frames: u32,
    validations: &crate::config::Video,
) -> Result<(), ValidationError> {
    if !validations.enable {
        return Err(ValidationError::VideoDisabled);
    }
    if width > validations.max_width {
        return Err(ValidationError::Width);
    }
    if height > validations.max_height {
        return Err(ValidationError::Height);
    }
    if u32::from(width) * u32::from(height) > validations.max_area {
        return Err(ValidationError::Area);
    }
    if frames > validations.max_frame_count {
        return Err(ValidationError::Frames);
    }
    if size > validations.max_file_size * MEGABYTES {
        return Err(ValidationError::Filesize);
    }

    Ok(())
}

#[tracing::instrument(skip(bytes, validations))]
async fn process_video(
    bytes: Bytes,
    input: InputVideoFormat,
    width: u16,
    height: u16,
    frames: u32,
    validations: &crate::config::Video,
    timeout: u64,
) -> Result<(InternalFormat, impl AsyncRead + Unpin), Error> {
    validate_video(bytes.len(), width, height, frames, validations)?;

    let output = input.build_output(
        validations.video_codec,
        validations.audio_codec,
        validations.allow_audio,
    );

    let crf = validations.crf_for(width, height);

    let read = ffmpeg::transcode_bytes(input, output, crf, timeout, bytes).await?;

    Ok((InternalFormat::Video(output.format.internal_format()), read))
}
