mod exiftool;
mod ffmpeg;
mod magick;

use crate::{
    bytes_stream::BytesStream,
    discover::Discovery,
    error::Error,
    error_code::ErrorCode,
    formats::{
        AnimationFormat, AnimationOutput, ImageInput, ImageOutput, InputFile, InputVideoFormat,
        InternalFormat,
    },
    future::WithPollTimer,
    process::{Process, ProcessRead},
    state::State,
    UploadLimits,
};

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

    #[error("Media type wasn't allowed for this upload")]
    MediaDisallowed,
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
            Self::MediaDisallowed => ErrorCode::MEDIA_DISALLOWED,
        }
    }
}

const MEGABYTES: usize = 1024 * 1024;

#[tracing::instrument(skip_all)]
pub(crate) async fn validate_bytes_stream<S>(
    state: &State<S>,
    bytes: BytesStream,
    upload_limits: &UploadLimits,
) -> Result<(InternalFormat, ProcessRead), Error> {
    if bytes.is_empty() {
        return Err(ValidationError::Empty.into());
    }

    let Discovery {
        input,
        width,
        height,
        frames,
    } = crate::discover::discover_bytes_stream(state, bytes.clone())
        .with_poll_timer("discover-bytes-stream")
        .await?;

    validate_upload(bytes.len(), width, height, frames, upload_limits)?;

    match &input {
        InputFile::Image(input) if *upload_limits.allow_image => {
            let (format, process) =
                process_image_command(state, *input, bytes.len(), width, height).await?;

            Ok((format, process.drive_with_stream(bytes.into_io_stream())))
        }
        InputFile::Animation(input) if *upload_limits.allow_animation => {
            let (format, process) = process_animation_command(
                state,
                *input,
                bytes.len(),
                width,
                height,
                frames.unwrap_or(1),
            )
            .await?;

            Ok((format, process.drive_with_stream(bytes.into_io_stream())))
        }
        InputFile::Video(input) if *upload_limits.allow_video => {
            let (format, process_read) =
                process_video(state, bytes, *input, width, height, frames.unwrap_or(1)).await?;

            Ok((format, process_read))
        }
        _ => Err(ValidationError::MediaDisallowed.into()),
    }
}

fn validate_upload(
    size: usize,
    width: u16,
    height: u16,
    frames: Option<u32>,
    upload_limits: &UploadLimits,
) -> Result<(), ValidationError> {
    if upload_limits
        .max_width
        .is_some_and(|max_width| width > *max_width)
    {
        return Err(ValidationError::Width);
    }

    if upload_limits
        .max_height
        .is_some_and(|max_height| height > *max_height)
    {
        return Err(ValidationError::Height);
    }

    if upload_limits
        .max_frame_count
        .zip(frames)
        .is_some_and(|(max_frame_count, frames)| frames > *max_frame_count)
    {
        return Err(ValidationError::Frames);
    }

    if upload_limits
        .max_area
        .is_some_and(|max_area| u32::from(width) * u32::from(height) > *max_area)
    {
        return Err(ValidationError::Area);
    }

    if upload_limits
        .max_file_size
        .is_some_and(|max_file_size| size > *max_file_size * MEGABYTES)
    {
        return Err(ValidationError::Filesize);
    }

    Ok(())
}

#[tracing::instrument(skip(state))]
async fn process_image_command<S>(
    state: &State<S>,
    input: ImageInput,
    size: usize,
    width: u16,
    height: u16,
) -> Result<(InternalFormat, Process), Error> {
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
    if size > validations.max_file_size * MEGABYTES {
        return Err(ValidationError::Filesize.into());
    }

    let ImageOutput {
        format,
        needs_transcode,
    } = input.build_output(validations.format);

    let process = if needs_transcode {
        let quality = validations.quality_for(format);

        magick::convert_image_command(state, input.format, format, quality).await?
    } else {
        exiftool::clear_metadata_command(state.config.media.process_timeout).await?
    };

    Ok((InternalFormat::Image(format), process))
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

#[tracing::instrument(skip(state))]
async fn process_animation_command<S>(
    state: &State<S>,
    input: AnimationFormat,
    size: usize,
    width: u16,
    height: u16,
    frames: u32,
) -> Result<(InternalFormat, Process), Error> {
    let validations = &state.config.media.animation;

    validate_animation(size, width, height, frames, validations)?;

    let AnimationOutput {
        format,
        needs_transcode,
    } = input.build_output(validations.format);

    let process = if needs_transcode {
        let quality = validations.quality_for(format);

        magick::convert_animation_command(state, input, format, quality).await?
    } else {
        exiftool::clear_metadata_command(state.config.media.process_timeout).await?
    };

    Ok((InternalFormat::Animation(format), process))
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

#[tracing::instrument(skip(state, bytes), fields(len = bytes.len()))]
async fn process_video<S>(
    state: &State<S>,
    bytes: BytesStream,
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
    .with_poll_timer("transcode-bytes")
    .await?;

    Ok((
        InternalFormat::Video(output.format.internal_format()),
        process_read,
    ))
}
