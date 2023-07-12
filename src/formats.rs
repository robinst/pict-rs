fn image_apng() -> mime::Mime {
    "image/apng".parse().unwrap()
}

fn image_avif() -> mime::Mime {
    "image/avif".parse().unwrap()
}

fn image_jxl() -> mime::Mime {
    "image/jxl".parse().unwrap()
}

fn image_webp() -> mime::Mime {
    "image/webp".parse().unwrap()
}

fn video_mp4() -> mime::Mime {
    "video/mp4".parse().unwrap()
}

fn video_webm() -> mime::Mime {
    "video/webm".parse().unwrap()
}

#[derive(Clone, Debug)]
pub(crate) struct PrescribedFormats {
    image: Option<ImageFormat>,
    animation: Option<AnimationFormat>,
    video: Option<OutputVideoFormat>,
    allow_audio: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) enum ImageFormat {
    #[serde(rename = "avif")]
    Avif,
    #[serde(rename = "png")]
    Png,
    #[serde(rename = "jpeg")]
    Jpeg,
    #[serde(rename = "jxl")]
    Jxl,
    #[serde(rename = "webp")]
    Webp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) enum AnimationFormat {
    #[serde(rename = "apng")]
    Apng,
    #[serde(rename = "avif")]
    Avif,
    #[serde(rename = "gif")]
    Gif,
    #[serde(rename = "webp")]
    Webp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum VideoFormat {
    Mp4,
    Webp { alpha: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) enum VideoCodec {
    #[serde(rename = "av1")]
    Av1,
    #[serde(rename = "h264")]
    H264,
    #[serde(rename = "h265")]
    H265,
    #[serde(rename = "vp8")]
    Vp8,
    #[serde(rename = "vp9")]
    Vp9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) enum AudioCodec {
    #[serde(rename = "aac")]
    Aac,
    #[serde(rename = "opus")]
    Opus,
    #[serde(rename = "vorbis")]
    Vorbis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum OutputVideoFormat {
    Mp4 {
        video_codec: Mp4Codec,
        audio_codec: Option<Mp4AudioCodec>,
    },
    Webm {
        video_codec: WebmCodec,
        audio_codec: Option<WebmAudioCodec>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) enum Mp4Codec {
    #[serde(rename = "h264")]
    H264,
    #[serde(rename = "h265")]
    H265,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) enum WebmAlphaCodec {
    #[serde(rename = "vp8")]
    Vp8,
    #[serde(rename = "vp9")]
    Vp9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AlphaCodec {
    pub(crate) alpha: bool,
    pub(crate) codec: WebmAlphaCodec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum WebmCodec {
    Av1,
    Alpha(AlphaCodec),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Mp4AudioCodec {
    Aac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum WebmAudioCodec {
    Opus,
    Vorbis,
}

#[derive(Clone, Debug)]
pub(crate) enum InputFile {
    Image {
        format: ImageFormat,
        needs_reorient: bool,
    },
    Animation(AnimationFormat),
    Video(VideoFormat),
}

#[derive(Clone, Debug)]
pub(crate) enum OutputFile {
    Image {
        format: ImageFormat,
        needs_transcode: bool,
    },
    Animation {
        format: AnimationFormat,
        needs_transcode: bool,
    },
    Video(OutputVideoFormat),
}

impl InputFile {
    const fn file_extension(&self) -> &'static str {
        match self {
            Self::Image { format, .. } => format.file_extension(),
            Self::Animation(format) => format.file_extension(),
            Self::Video(format) => format.file_extension(),
        }
    }

    const fn build_output(&self, prescribed: &PrescribedFormats) -> OutputFile {
        match (self, prescribed) {
            (
                InputFile::Image {
                    format,
                    needs_reorient,
                },
                PrescribedFormats {
                    image: Some(prescribed),
                    ..
                },
            ) => OutputFile::Image {
                format: *prescribed,
                needs_transcode: *needs_reorient || !format.const_eq(*prescribed),
            },
            (
                InputFile::Animation(format),
                PrescribedFormats {
                    animation: Some(prescribed),
                    ..
                },
            ) => OutputFile::Animation {
                format: *prescribed,
                needs_transcode: !format.const_eq(*prescribed),
            },
            (
                InputFile::Video(VideoFormat::Webp { alpha }),
                PrescribedFormats {
                    video:
                        Some(OutputVideoFormat::Webm {
                            video_codec: WebmCodec::Alpha(AlphaCodec { codec, .. }),
                            audio_codec,
                        }),
                    ..
                },
            ) => OutputFile::Video(OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: *alpha,
                    codec: *codec,
                }),
                audio_codec: *audio_codec,
            }),
            (
                InputFile::Video(_),
                PrescribedFormats {
                    video: Some(prescribed),
                    ..
                },
            ) => OutputFile::Video(*prescribed),
            (
                InputFile::Image {
                    format,
                    needs_reorient,
                },
                PrescribedFormats { image: None, .. },
            ) => OutputFile::Image {
                format: *format,
                needs_transcode: *needs_reorient,
            },
            (
                InputFile::Animation(input),
                PrescribedFormats {
                    animation: None, ..
                },
            ) => OutputFile::Animation {
                format: *input,
                needs_transcode: false,
            },
            (
                InputFile::Video(input),
                PrescribedFormats {
                    video: None,
                    allow_audio: true,
                    ..
                },
            ) => match input {
                VideoFormat::Mp4 => OutputFile::Video(OutputVideoFormat::Mp4 {
                    video_codec: Mp4Codec::H264,
                    audio_codec: Some(Mp4AudioCodec::Aac),
                }),
                VideoFormat::Webp { alpha } => OutputFile::Video(OutputVideoFormat::Webm {
                    video_codec: WebmCodec::Alpha(AlphaCodec {
                        alpha: *alpha,
                        codec: WebmAlphaCodec::Vp9,
                    }),
                    audio_codec: Some(WebmAudioCodec::Opus),
                }),
            },
            (
                InputFile::Video(input),
                PrescribedFormats {
                    video: None,
                    allow_audio: false,
                    ..
                },
            ) => match input {
                VideoFormat::Mp4 => OutputFile::Video(OutputVideoFormat::Mp4 {
                    video_codec: Mp4Codec::H264,
                    audio_codec: None,
                }),
                VideoFormat::Webp { alpha } => OutputFile::Video(OutputVideoFormat::Webm {
                    video_codec: WebmCodec::Alpha(AlphaCodec {
                        alpha: *alpha,
                        codec: WebmAlphaCodec::Vp9,
                    }),
                    audio_codec: None,
                }),
            },
        }
    }
}

impl ImageFormat {
    const fn const_eq(self, rhs: Self) -> bool {
        match (self, rhs) {
            (Self::Avif, Self::Avif)
            | (Self::Jpeg, Self::Jpeg)
            | (Self::Jxl, Self::Jxl)
            | (Self::Png, Self::Png)
            | (Self::Webp, Self::Webp) => true,
            (Self::Avif, _)
            | (Self::Jpeg, _)
            | (Self::Jxl, _)
            | (Self::Png, _)
            | (Self::Webp, _) => false,
        }
    }

    const fn file_extension(self) -> &'static str {
        match self {
            Self::Avif => ".avif",
            Self::Jpeg => ".jpeg",
            Self::Jxl => ".jxl",
            Self::Png => ".png",
            Self::Webp => ".webp",
        }
    }

    const fn magick_format(self) -> &'static str {
        match self {
            Self::Avif => "AVIF",
            Self::Jpeg => "JPEG",
            Self::Jxl => "JXL",
            Self::Png => "PNG",
            Self::Webp => "Webp",
        }
    }

    fn media_type(self) -> mime::Mime {
        match self {
            Self::Avif => image_avif(),
            Self::Jpeg => mime::IMAGE_JPEG,
            Self::Jxl => image_jxl(),
            Self::Png => mime::IMAGE_PNG,
            Self::Webp => image_webp(),
        }
    }
}

impl AnimationFormat {
    const fn const_eq(self, rhs: Self) -> bool {
        match (self, rhs) {
            (Self::Apng, Self::Apng)
            | (Self::Avif, Self::Avif)
            | (Self::Gif, Self::Gif)
            | (Self::Webp, Self::Webp) => true,
            (Self::Apng, _) | (Self::Avif, _) | (Self::Gif, _) | (Self::Webp, _) => false,
        }
    }

    const fn file_extension(self) -> &'static str {
        match self {
            Self::Apng => ".apng",
            Self::Avif => ".avif",
            Self::Gif => ".gif",
            Self::Webp => ".webp",
        }
    }

    const fn magick_format(self) -> &'static str {
        match self {
            Self::Apng => "APNG",
            Self::Avif => "AVIF",
            Self::Gif => "GIF",
            Self::Webp => "WEBP",
        }
    }

    fn media_type(self) -> mime::Mime {
        match self {
            Self::Apng => image_apng(),
            Self::Avif => image_avif(),
            Self::Gif => mime::IMAGE_GIF,
            Self::Webp => image_webp(),
        }
    }
}

impl VideoFormat {
    const fn file_extension(self) -> &'static str {
        match self {
            Self::Mp4 => ".mp4",
            Self::Webp { .. } => ".webm",
        }
    }
}

impl OutputVideoFormat {
    const fn from_parts(
        video_codec: VideoCodec,
        audio_codec: Option<AudioCodec>,
        allow_audio: bool,
    ) -> Self {
        match (video_codec, audio_codec) {
            (VideoCodec::Av1, Some(AudioCodec::Vorbis)) if allow_audio => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Av1,
                audio_codec: Some(WebmAudioCodec::Vorbis),
            },
            (VideoCodec::Av1, _) if allow_audio => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Av1,
                audio_codec: Some(WebmAudioCodec::Opus),
            },
            (VideoCodec::Av1, _) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Av1,
                audio_codec: None,
            },
            (VideoCodec::H264, _) if allow_audio => OutputVideoFormat::Mp4 {
                video_codec: Mp4Codec::H264,
                audio_codec: Some(Mp4AudioCodec::Aac),
            },
            (VideoCodec::H264, _) => OutputVideoFormat::Mp4 {
                video_codec: Mp4Codec::H264,
                audio_codec: None,
            },
            (VideoCodec::H265, _) if allow_audio => OutputVideoFormat::Mp4 {
                video_codec: Mp4Codec::H265,
                audio_codec: Some(Mp4AudioCodec::Aac),
            },
            (VideoCodec::H265, _) => OutputVideoFormat::Mp4 {
                video_codec: Mp4Codec::H265,
                audio_codec: None,
            },
            (VideoCodec::Vp8, Some(AudioCodec::Vorbis)) if allow_audio => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp8,
                }),
                audio_codec: Some(WebmAudioCodec::Vorbis),
            },
            (VideoCodec::Vp8, _) if allow_audio => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp8,
                }),
                audio_codec: Some(WebmAudioCodec::Opus),
            },
            (VideoCodec::Vp8, _) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp8,
                }),
                audio_codec: None,
            },
            (VideoCodec::Vp9, Some(AudioCodec::Vorbis)) if allow_audio => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp9,
                }),
                audio_codec: Some(WebmAudioCodec::Vorbis),
            },
            (VideoCodec::Vp9, _) if allow_audio => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp9,
                }),
                audio_codec: Some(WebmAudioCodec::Opus),
            },
            (VideoCodec::Vp9, _) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp9,
                }),
                audio_codec: None,
            },
        }
    }

    const fn file_extension(self) -> &'static str {
        match self {
            Self::Mp4 { .. } => ".mp4",
            Self::Webm { .. } => ".webm",
        }
    }

    const fn ffmpeg_format(self) -> &'static str {
        match self {
            Self::Mp4 { .. } => "mp4",
            Self::Webm { .. } => "webm",
        }
    }

    const fn ffmpeg_video_codec(self) -> &'static str {
        match self {
            Self::Mp4 { video_codec, .. } => video_codec.ffmpeg_codec(),
            Self::Webm { video_codec, .. } => video_codec.ffmpeg_codec(),
        }
    }

    const fn ffmpeg_audio_codec(self) -> Option<&'static str> {
        match self {
            Self::Mp4 {
                audio_codec: Some(audio_codec),
                ..
            } => Some(audio_codec.ffmpeg_codec()),
            Self::Webm {
                audio_codec: Some(audio_codec),
                ..
            } => Some(audio_codec.ffmpeg_codec()),
            _ => None,
        }
    }

    const fn pix_fmt(self) -> &'static str {
        match self {
            Self::Mp4 { .. } => "yuv420p",
            Self::Webm { video_codec, .. } => video_codec.pix_fmt(),
        }
    }

    fn media_type(self) -> mime::Mime {
        match self {
            Self::Mp4 { .. } => video_mp4(),
            Self::Webm { .. } => video_webm(),
        }
    }
}

impl Mp4Codec {
    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "hevc",
        }
    }
}

impl WebmAlphaCodec {
    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp9",
        }
    }
}

impl WebmCodec {
    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Av1 => "av1",
            Self::Alpha(AlphaCodec { codec, .. }) => codec.ffmpeg_codec(),
        }
    }

    const fn pix_fmt(self) -> &'static str {
        match self {
            Self::Alpha(AlphaCodec { alpha: true, .. }) => "yuva420p",
            _ => "yuv420p",
        }
    }
}

impl Mp4AudioCodec {
    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Aac => "aac",
        }
    }
}

impl WebmAudioCodec {
    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Opus => "libopus",
            Self::Vorbis => "vorbis",
        }
    }
}

impl OutputFile {
    const fn file_extension(&self) -> &'static str {
        match self {
            Self::Image { format, .. } => format.file_extension(),
            Self::Animation { format, .. } => format.file_extension(),
            Self::Video(format) => format.file_extension(),
        }
    }

    fn media_type(&self) -> mime::Mime {
        match self {
            Self::Image { format, .. } => format.media_type(),
            Self::Animation { format, .. } => format.media_type(),
            Self::Video(format) => format.media_type(),
        }
    }
}
