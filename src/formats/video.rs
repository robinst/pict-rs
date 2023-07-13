#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum VideoFormat {
    Mp4,
    Webm { alpha: bool },
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
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

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    clap::ValueEnum,
)]
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

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    clap::ValueEnum,
)]
pub(crate) enum AudioCodec {
    #[serde(rename = "aac")]
    Aac,
    #[serde(rename = "opus")]
    Opus,
    #[serde(rename = "vorbis")]
    Vorbis,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub(crate) enum Mp4Codec {
    #[serde(rename = "h264")]
    H264,
    #[serde(rename = "h265")]
    H265,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub(crate) enum WebmAlphaCodec {
    #[serde(rename = "vp8")]
    Vp8,
    #[serde(rename = "vp9")]
    Vp9,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub(crate) struct AlphaCodec {
    pub(crate) alpha: bool,
    pub(crate) codec: WebmAlphaCodec,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub(crate) enum WebmCodec {
    Av1,
    Alpha(AlphaCodec),
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub(crate) enum Mp4AudioCodec {
    Aac,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub(crate) enum WebmAudioCodec {
    Opus,
    Vorbis,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub(crate) enum InternalVideoFormat {
    Mp4,
    Webm,
}

impl VideoFormat {
    pub(crate) const fn ffmpeg_format(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm { .. } => "webm",
        }
    }

    pub(crate) const fn internal_format(self) -> InternalVideoFormat {
        match self {
            Self::Mp4 => InternalVideoFormat::Mp4,
            Self::Webm { .. } => InternalVideoFormat::Webm,
        }
    }

    pub(crate) const fn build_output(
        self,
        video_codec: VideoCodec,
        audio_codec: Option<AudioCodec>,
        allow_audio: bool,
    ) -> OutputVideoFormat {
        match (video_codec, self) {
            (VideoCodec::Vp8, Self::Webm { alpha }) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha,
                    codec: WebmAlphaCodec::Vp8,
                }),
                audio_codec: if allow_audio {
                    match audio_codec {
                        Some(AudioCodec::Vorbis) => Some(WebmAudioCodec::Vorbis),
                        _ => Some(WebmAudioCodec::Opus),
                    }
                } else {
                    None
                },
            },
            (VideoCodec::Vp8, _) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp8,
                }),
                audio_codec: if allow_audio {
                    match audio_codec {
                        Some(AudioCodec::Vorbis) => Some(WebmAudioCodec::Vorbis),
                        _ => Some(WebmAudioCodec::Opus),
                    }
                } else {
                    None
                },
            },
            (VideoCodec::Vp9, Self::Webm { alpha }) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha,
                    codec: WebmAlphaCodec::Vp9,
                }),
                audio_codec: if allow_audio {
                    match audio_codec {
                        Some(AudioCodec::Vorbis) => Some(WebmAudioCodec::Vorbis),
                        _ => Some(WebmAudioCodec::Opus),
                    }
                } else {
                    None
                },
            },
            (VideoCodec::Vp9, _) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Alpha(AlphaCodec {
                    alpha: false,
                    codec: WebmAlphaCodec::Vp9,
                }),
                audio_codec: if allow_audio {
                    match audio_codec {
                        Some(AudioCodec::Vorbis) => Some(WebmAudioCodec::Vorbis),
                        _ => Some(WebmAudioCodec::Opus),
                    }
                } else {
                    None
                },
            },
            (VideoCodec::Av1, _) => OutputVideoFormat::Webm {
                video_codec: WebmCodec::Av1,
                audio_codec: if allow_audio {
                    match audio_codec {
                        Some(AudioCodec::Vorbis) => Some(WebmAudioCodec::Vorbis),
                        _ => Some(WebmAudioCodec::Opus),
                    }
                } else {
                    None
                },
            },
            (VideoCodec::H264, _) => OutputVideoFormat::Mp4 {
                video_codec: Mp4Codec::H264,
                audio_codec: if allow_audio {
                    Some(Mp4AudioCodec::Aac)
                } else {
                    None
                },
            },
            (VideoCodec::H265, _) => OutputVideoFormat::Mp4 {
                video_codec: Mp4Codec::H265,
                audio_codec: if allow_audio {
                    Some(Mp4AudioCodec::Aac)
                } else {
                    None
                },
            },
        }
    }
}

impl OutputVideoFormat {
    pub(crate) const fn from_parts(
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

    pub(crate) const fn magick_format(self) -> &'static str {
        match self {
            Self::Mp4 { .. } => "MP4",
            Self::Webm { .. } => "WEBM",
        }
    }

    pub(crate) const fn ffmpeg_format(self) -> &'static str {
        match self {
            Self::Mp4 { .. } => "mp4",
            Self::Webm { .. } => "webm",
        }
    }

    pub(crate) const fn ffmpeg_video_codec(self) -> &'static str {
        match self {
            Self::Mp4 { video_codec, .. } => video_codec.ffmpeg_codec(),
            Self::Webm { video_codec, .. } => video_codec.ffmpeg_codec(),
        }
    }

    pub(crate) const fn ffmpeg_audio_codec(self) -> Option<&'static str> {
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

    pub(crate) const fn pix_fmt(self) -> &'static str {
        match self {
            Self::Mp4 { .. } => "yuv420p",
            Self::Webm { video_codec, .. } => video_codec.pix_fmt(),
        }
    }

    pub(crate) const fn internal_format(self) -> InternalVideoFormat {
        match self {
            Self::Mp4 { .. } => InternalVideoFormat::Mp4,
            Self::Webm { .. } => InternalVideoFormat::Webm,
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

impl InternalVideoFormat {
    pub(crate) const fn file_extension(self) -> &'static str {
        match self {
            Self::Mp4 => ".mp4",
            Self::Webm => ".webm",
        }
    }

    pub(super) fn media_type(self) -> mime::Mime {
        match self {
            Self::Mp4 => super::mimes::video_mp4(),
            Self::Webm => super::mimes::video_webm(),
        }
    }
}
