mod animation;
mod image;
pub(crate) mod mimes;
mod video;

use std::str::FromStr;

pub(crate) use animation::{AnimationFormat, AnimationOutput};
pub(crate) use image::{ImageFormat, ImageInput, ImageOutput};
pub(crate) use video::{
    AlphaCodec, AudioCodec, InputVideoFormat, InternalVideoFormat, Mp4AudioCodec, Mp4Codec,
    OutputVideo, VideoCodec, WebmAlphaCodec, WebmAudioCodec, WebmCodec,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InputFile {
    Image(ImageInput),
    Animation(AnimationFormat),
    Video(InputVideoFormat),
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub(crate) enum InternalFormat {
    Animation(AnimationFormat),
    Image(ImageFormat),
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
            Self::Animation(format) => InternalFormat::Animation(*format),
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

    pub(crate) const fn to_byte(self) -> u8 {
        match self {
            Self::Animation(AnimationFormat::Apng) => 0,
            Self::Animation(AnimationFormat::Avif) => 1,
            Self::Animation(AnimationFormat::Gif) => 2,
            Self::Animation(AnimationFormat::Webp) => 3,
            Self::Image(ImageFormat::Avif) => 4,
            Self::Image(ImageFormat::Jpeg) => 5,
            Self::Image(ImageFormat::Jxl) => 6,
            Self::Image(ImageFormat::Png) => 7,
            Self::Image(ImageFormat::Webp) => 8,
            Self::Video(InternalVideoFormat::Mp4) => 9,
            Self::Video(InternalVideoFormat::Webm) => 10,
        }
    }

    pub(crate) const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Animation(AnimationFormat::Apng)),
            1 => Some(Self::Animation(AnimationFormat::Avif)),
            2 => Some(Self::Animation(AnimationFormat::Gif)),
            3 => Some(Self::Animation(AnimationFormat::Webp)),
            4 => Some(Self::Image(ImageFormat::Avif)),
            5 => Some(Self::Image(ImageFormat::Jpeg)),
            6 => Some(Self::Image(ImageFormat::Jxl)),
            7 => Some(Self::Image(ImageFormat::Png)),
            8 => Some(Self::Image(ImageFormat::Webp)),
            9 => Some(Self::Video(InternalVideoFormat::Mp4)),
            10 => Some(Self::Video(InternalVideoFormat::Webm)),
            _ => None,
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

    pub(crate) const fn process_to(self, output: InputProcessableFormat) -> Self {
        match (self, output) {
            (Self::Image(_), InputProcessableFormat::Avif) => Self::Image(ImageFormat::Avif),
            (Self::Image(_) | Self::Animation(_), InputProcessableFormat::Jpeg) => {
                Self::Image(ImageFormat::Jpeg)
            }
            (Self::Image(_) | Self::Animation(_), InputProcessableFormat::Jxl) => {
                Self::Image(ImageFormat::Jxl)
            }
            (Self::Image(_) | Self::Animation(_), InputProcessableFormat::Png) => {
                Self::Image(ImageFormat::Png)
            }
            (Self::Image(_), InputProcessableFormat::Webp) => Self::Image(ImageFormat::Webp),
            (Self::Animation(_) | Self::Image(_), InputProcessableFormat::Apng) => {
                Self::Animation(AnimationFormat::Apng)
            }
            (Self::Animation(_), InputProcessableFormat::Avif) => {
                Self::Animation(AnimationFormat::Avif)
            }
            (Self::Animation(_) | Self::Image(_), InputProcessableFormat::Gif) => {
                Self::Animation(AnimationFormat::Gif)
            }
            (Self::Animation(_), InputProcessableFormat::Webp) => {
                Self::Animation(AnimationFormat::Webp)
            }
        }
    }

    pub(crate) const fn should_thumbnail(self, output: Self) -> bool {
        matches!((self, output), (Self::Animation(_), Self::Image(_)))
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
