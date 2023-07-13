mod animation;
mod image;
pub(crate) mod mimes;
mod video;

use std::str::FromStr;

pub(crate) use animation::{AnimationFormat, AnimationInput, AnimationOutput};
pub(crate) use image::{ImageFormat, ImageInput, ImageOutput};
pub(crate) use video::{InternalVideoFormat, OutputVideoFormat, VideoFormat};

#[derive(Clone, Debug)]
pub(crate) struct PrescribedFormats {
    pub(crate) image: Option<ImageFormat>,
    pub(crate) animation: Option<AnimationFormat>,
    pub(crate) video: Option<OutputVideoFormat>,
    pub(crate) allow_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputFile {
    Image(ImageInput),
    Animation(AnimationInput),
    Video(VideoFormat),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum InternalFormat {
    Image(ImageFormat),
    Animation(AnimationFormat),
    Video(InternalVideoFormat),
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum ProcessableFormat {
    Image(ImageFormat),
    Animation(AnimationFormat),
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) enum InputProcessableFormat {
    #[serde(rename = "apng")]
    Apng,
    #[serde(rename = "avif")]
    Avif,
    #[serde(rename = "gif")]
    Gif,
    #[serde(rename = "jpeg")]
    Jpeg,
    #[serde(rename = "jxl")]
    Jxl,
    #[serde(rename = "png")]
    Png,
    #[serde(rename = "webp")]
    Webp,
}

impl InputFile {
    pub(crate) const fn internal_format(&self) -> InternalFormat {
        match self {
            Self::Image(ImageInput { format, .. }) => InternalFormat::Image(*format),
            Self::Animation(AnimationInput { format }) => InternalFormat::Animation(*format),
            Self::Video(format) => InternalFormat::Video(format.internal_format()),
        }
    }
}

impl InternalFormat {
    pub(crate) fn media_type(self) -> mime::Mime {
        match self {
            Self::Image(format) => format.media_type(),
            Self::Animation(format) => format.media_type(),
            Self::Video(format) => format.media_type(),
        }
    }

    pub(crate) fn maybe_from_media_type(mime: &mime::Mime, has_frames: bool) -> Option<Self> {
        match (mime.type_(), mime.subtype().as_str(), has_frames) {
            (mime::IMAGE, "apng", _) => Some(Self::Animation(AnimationFormat::Apng)),
            (mime::IMAGE, "avif", true) => Some(Self::Animation(AnimationFormat::Avif)),
            (mime::IMAGE, "avif", false) => Some(Self::Image(ImageFormat::Avif)),
            (mime::IMAGE, "gif", _) => Some(Self::Animation(AnimationFormat::Gif)),
            (mime::IMAGE, "jpeg", _) => Some(Self::Image(ImageFormat::Jpeg)),
            (mime::IMAGE, "jxl", _) => Some(Self::Image(ImageFormat::Jxl)),
            (mime::IMAGE, "png", _) => Some(Self::Image(ImageFormat::Png)),
            (mime::IMAGE, "webp", true) => Some(Self::Animation(AnimationFormat::Webp)),
            (mime::IMAGE, "webp", false) => Some(Self::Image(ImageFormat::Webp)),
            (mime::VIDEO, "mp4", _) => Some(Self::Video(InternalVideoFormat::Mp4)),
            (mime::VIDEO, "webm", _) => Some(Self::Video(InternalVideoFormat::Webm)),
            _ => None,
        }
    }

    pub(crate) const fn file_extension(self) -> &'static str {
        match self {
            Self::Image(format) => format.file_extension(),
            Self::Animation(format) => format.file_extension(),
            Self::Video(format) => format.file_extension(),
        }
    }

    pub(crate) const fn processable_format(self) -> Option<ProcessableFormat> {
        match self {
            Self::Image(format) => Some(ProcessableFormat::Image(format)),
            Self::Animation(format) => Some(ProcessableFormat::Animation(format)),
            Self::Video(_) => None,
        }
    }
}

impl ProcessableFormat {
    pub(crate) const fn file_extension(self) -> &'static str {
        match self {
            Self::Image(format) => format.file_extension(),
            Self::Animation(format) => format.file_extension(),
        }
    }

    pub(crate) const fn coalesce(self) -> bool {
        matches!(self, Self::Animation(_))
    }

    pub(crate) fn magick_format(self) -> &'static str {
        match self {
            Self::Image(format) => format.magick_format(),
            Self::Animation(format) => format.magick_format(),
        }
    }

    pub(crate) fn process_to(self, output: InputProcessableFormat) -> Option<Self> {
        match (self, output) {
            (Self::Image(_), InputProcessableFormat::Avif) => Some(Self::Image(ImageFormat::Avif)),
            (Self::Image(_), InputProcessableFormat::Jpeg) => Some(Self::Image(ImageFormat::Jpeg)),
            (Self::Image(_), InputProcessableFormat::Jxl) => Some(Self::Image(ImageFormat::Jxl)),
            (Self::Image(_), InputProcessableFormat::Png) => Some(Self::Image(ImageFormat::Png)),
            (Self::Image(_), InputProcessableFormat::Webp) => Some(Self::Image(ImageFormat::Webp)),
            (Self::Animation(_), InputProcessableFormat::Apng) => {
                Some(Self::Animation(AnimationFormat::Apng))
            }
            (Self::Animation(_), InputProcessableFormat::Avif) => {
                Some(Self::Animation(AnimationFormat::Avif))
            }
            (Self::Animation(_), InputProcessableFormat::Gif) => {
                Some(Self::Animation(AnimationFormat::Gif))
            }
            (Self::Animation(_), InputProcessableFormat::Webp) => {
                Some(Self::Animation(AnimationFormat::Webp))
            }
            (Self::Image(_), InputProcessableFormat::Apng) => None,
            (Self::Image(_), InputProcessableFormat::Gif) => None,
            (Self::Animation(_), InputProcessableFormat::Jpeg) => None,
            (Self::Animation(_), InputProcessableFormat::Jxl) => None,
            (Self::Animation(_), InputProcessableFormat::Png) => None,
        }
    }
}

impl FromStr for InputProcessableFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "apng" => Ok(Self::Apng),
            "avif" => Ok(Self::Avif),
            "gif" => Ok(Self::Gif),
            "jpeg" => Ok(Self::Jpeg),
            "jpg" => Ok(Self::Jpeg),
            "jxl" => Ok(Self::Jxl),
            "png" => Ok(Self::Png),
            "webp" => Ok(Self::Webp),
            otherwise => Err(format!("Invalid format: {otherwise}")),
        }
    }
}

impl std::fmt::Display for InputProcessableFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Apng => write!(f, "apng"),
            Self::Avif => write!(f, "avif"),
            Self::Gif => write!(f, "gif"),
            Self::Jpeg => write!(f, "jpeg"),
            Self::Jxl => write!(f, "jxl"),
            Self::Png => write!(f, "png"),
            Self::Webp => write!(f, "webp"),
        }
    }
}
