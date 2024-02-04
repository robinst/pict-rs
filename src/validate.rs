mod exiftool;
mod ffmpeg;
mod magick;

use crate::{
    discover::Discovery,
    error::Error,
    error_code::ErrorCode,
    formats::{
        AnimationFormat, AnimationOutput, ImageInput, ImageOutput, InputFile, InputVideoFormat,
        InternalFormat,
    },
    process::ProcessRead,
    state::State,
};
use actix_web::web::Bytes;

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

impl ValidationError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Width => ErrorCode::VALIDATE_WIDTH,
            Self::Height => ErrorCode::VALIDATE_HEIGHT,
            Self::Area => ErrorCode::VALIDATE_AREA,
            Self::Frames => ErrorCode::VALIDATE_FRAMES,
            Self::Empty => ErrorCode::VALIDATE_FILE_EMPTY,
            Self::Filesize => ErrorCode::VALIDATE_FILE_SIZE,
            Self::VideoDisabled => ErrorCode::VIDEO_DISABLED,
        }
    }
}

const MEGABYTES: usize = 1024 * 1024;

#[tracing::instrument(skip_all)]
pub(crate) async fn validate_bytes<S>(
    state: &State<S>,
    bytes: Bytes,
) -> Result<(InternalFormat, ProcessRead), Error> {
    if bytes.is_empty() {
        return Err(ValidationError::Empty.into());
    }

    let Discovery {
        input,
        width,
        height,
        frames,
    } = crate::discover::discover_bytes(state, bytes.clone()).await?;

    match &input {
        InputFile::Image(input) => {
            let (format, process_read) = process_image(state, bytes, *input, width, height).await?;

            Ok((format, process_read))
        }
        InputFile::Animation(input) => {
            let (format, process_read) =
                process_animation(state, bytes, *input, width, height, frames.unwrap_or(1)).await?;

            Ok((format, process_read))
        }
        InputFile::Video(input) => {
            let (format, process_read) =
                process_video(state, bytes, *input, width, height, frames.unwrap_or(1)).await?;

            Ok((format, process_read))
        }
    }
}

#[tracing::instrument(skip(state, bytes))]
async fn process_image<S>(
    state: &State<S>,
    bytes: Bytes,
    input: ImageInput,
    width: u16,
    height: u16,
) -> Result<(InternalFormat, ProcessRead), Error> {
    let validations = &state.config.media.image;

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

    let process_read = if needs_transcode {
        let quality = validations.quality_for(format);

        magick::convert_image(state, input.format, format, quality, bytes).await?
    } else {
        exiftool::clear_metadata_bytes_read(bytes, state.config.media.process_timeout)?
    };

    Ok((InternalFormat::Image(format), process_read))
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

#[tracing::instrument(skip(state, bytes))]
async fn process_animation<S>(
    state: &State<S>,
    bytes: Bytes,
    input: AnimationFormat,
    width: u16,
    height: u16,
    frames: u32,
) -> Result<(InternalFormat, ProcessRead), Error> {
    let validations = &state.config.media.animation;

    validate_animation(bytes.len(), width, height, frames, validations)?;

    let AnimationOutput {
        format,
        needs_transcode,
    } = input.build_output(validations.format);

    let process_read = if needs_transcode {
        let quality = validations.quality_for(format);

        magick::convert_animation(state, input, format, quality, bytes).await?
    } else {
        exiftool::clear_metadata_bytes_read(bytes, state.config.media.process_timeout)?
    };

    Ok((InternalFormat::Animation(format), process_read))
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

#[tracing::instrument(skip(state, bytes))]
async fn process_video<S>(
    state: &State<S>,
    bytes: Bytes,
    input: InputVideoFormat,
    width: u16,
    height: u16,
    frames: u32,
) -> Result<(InternalFormat, ProcessRead), Error> {
    let validations = &state.config.media.video;

    validate_video(bytes.len(), width, height, frames, validations)?;

    let output = input.build_output(
        validations.video_codec,
        validations.audio_codec,
        validations.allow_audio,
    );

    let crf = validations.crf_for(width, height);

    let process_read = ffmpeg::transcode_bytes(
        &state.tmp_dir,
        input,
        output,
        crf,
        state.config.media.process_timeout,
        bytes,
    )
    .await?;

    Ok((
        InternalFormat::Video(output.format.internal_format()),
        process_read,
    ))
}
