mod exiftool;
mod ffmpeg;
mod magick;

use actix_web::web::Bytes;

use crate::formats::{AnimationInput, ImageInput, InputFile, InternalFormat, InternalVideoFormat};

pub(crate) struct Discovery {
    pub(crate) input: InputFile,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) frames: Option<u32>,
}

impl Discovery {
    pub(crate) fn internal_format(&self) -> InternalFormat {
        match self.input {
            InputFile::Image(ImageInput { format, .. }) => InternalFormat::Image(format),
            InputFile::Animation(AnimationInput { format }) => InternalFormat::Animation(format),
            // we're making up bs now lol
            InputFile::Video(crate::formats::VideoFormat::Mp4) => {
                InternalFormat::Video(InternalVideoFormat::Mp4)
            }
            InputFile::Video(crate::formats::VideoFormat::Webm { .. }) => {
                InternalFormat::Video(InternalVideoFormat::Webm)
            }
        }
    }
}

pub(crate) async fn discover_bytes(bytes: Bytes) -> Result<Discovery, crate::error::Error> {
    let discovery = ffmpeg::discover_bytes(bytes.clone()).await?;

    let discovery = magick::confirm_bytes(discovery, bytes.clone()).await?;

    let discovery = exiftool::check_reorient(discovery, bytes).await?;

    Ok(discovery)
}
