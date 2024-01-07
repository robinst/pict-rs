#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputVideoFormat {
    Mp4 {
        video_codec: Mp4Codec,
        audio_codec: Option<Mp4AudioCodec>,
    },
    Webm {
        video_codec: WebmCodec,
        audio_codec: Option<WebmAudioCodec>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct OutputVideo {
    pub(crate) transcode_video: bool,
    pub(crate) transcode_audio: bool,
    pub(crate) format: OutputVideoFormat,
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
    #[serde(rename = "av1")]
    Av1,
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

const fn webm_audio(
    allow_audio: bool,
    has_audio: bool,
    prescribed: Option<AudioCodec>,
    provided: Option<WebmAudioCodec>,
) -> (Option<WebmAudioCodec>, bool) {
    if allow_audio && has_audio {
        match prescribed {
            Some(AudioCodec::Opus) => (
                Some(WebmAudioCodec::Opus),
                !matches!(provided, Some(WebmAudioCodec::Opus)),
            ),
            Some(AudioCodec::Vorbis) => (
                Some(WebmAudioCodec::Vorbis),
                !matches!(provided, Some(WebmAudioCodec::Vorbis)),
            ),
            _ => match provided {
                Some(codec) => (Some(codec), false),
                None => (Some(WebmAudioCodec::Opus), true),
            },
        }
    } else {
        (None, true)
    }
}

const fn mp4_audio(
    allow_audio: bool,
    has_audio: bool,
    prescribed: Option<AudioCodec>,
    provided: Option<Mp4AudioCodec>,
) -> (Option<Mp4AudioCodec>, bool) {
    if allow_audio && has_audio {
        match prescribed {
            Some(AudioCodec::Aac) => (
                Some(Mp4AudioCodec::Aac),
                !matches!(provided, Some(Mp4AudioCodec::Aac)),
            ),
            _ => match provided {
                Some(codec) => (Some(codec), false),
                None => (Some(Mp4AudioCodec::Aac), true),
            },
        }
    } else {
        (None, true)
    }
}

impl InputVideoFormat {
    pub(crate) const fn internal_format(self) -> InternalVideoFormat {
        match self {
            Self::Mp4 { .. } => InternalVideoFormat::Mp4,
            Self::Webm { .. } => InternalVideoFormat::Webm,
        }
    }

    const fn transcode_vorbis(
        self,
        prescribed_codec: WebmAlphaCodec,
        prescribed_audio_codec: Option<AudioCodec>,
        allow_audio: bool,
    ) -> OutputVideo {
        match self {
            Self::Webm {
                video_codec,
                audio_codec,
            } => {
                let (audio_codec, transcode_audio) = webm_audio(
                    allow_audio,
                    audio_codec.is_some(),
                    prescribed_audio_codec,
                    audio_codec,
                );

                let (alpha, transcode_video) = match video_codec {
                    WebmCodec::Alpha(AlphaCodec { alpha, codec }) => {
                        (alpha, !codec.const_eq(prescribed_codec))
                    }
                    WebmCodec::Av1 => (false, true),
                };

                OutputVideo {
                    format: OutputVideoFormat::Webm {
                        video_codec: WebmCodec::Alpha(AlphaCodec {
                            alpha,
                            codec: prescribed_codec,
                        }),
                        audio_codec,
                    },
                    transcode_video,
                    transcode_audio,
                }
            }
            Self::Mp4 { audio_codec, .. } => {
                let (audio_codec, transcode_audio) = webm_audio(
                    allow_audio,
                    audio_codec.is_some(),
                    prescribed_audio_codec,
                    None,
                );

                OutputVideo {
                    format: OutputVideoFormat::Webm {
                        video_codec: WebmCodec::Alpha(AlphaCodec {
                            alpha: false,
                            codec: prescribed_codec,
                        }),
                        audio_codec,
                    },
                    transcode_video: true,
                    transcode_audio,
                }
            }
        }
    }

    const fn transcode_av1(
        self,
        prescribed_audio_codec: Option<AudioCodec>,
        allow_audio: bool,
    ) -> OutputVideo {
        match self {
            Self::Webm {
                video_codec,
                audio_codec,
            } => {
                let (audio_codec, transcode_audio) = webm_audio(
                    allow_audio,
                    audio_codec.is_some(),
                    prescribed_audio_codec,
                    audio_codec,
                );

                OutputVideo {
                    format: OutputVideoFormat::Webm {
                        video_codec: WebmCodec::Av1,
                        audio_codec,
                    },
                    transcode_video: !video_codec.const_eq(WebmCodec::Av1),
                    transcode_audio,
                }
            }
            Self::Mp4 {
                video_codec,
                audio_codec,
            } => {
                let (audio_codec, transcode_audio) = webm_audio(
                    allow_audio,
                    audio_codec.is_some(),
                    prescribed_audio_codec,
                    None,
                );

                OutputVideo {
                    format: OutputVideoFormat::Webm {
                        video_codec: WebmCodec::Av1,
                        audio_codec,
                    },
                    transcode_video: !video_codec.is_av1(),
                    transcode_audio,
                }
            }
        }
    }

    const fn transcode_mp4(
        self,
        prescribed_codec: Mp4Codec,
        prescribed_audio_codec: Option<AudioCodec>,
        allow_audio: bool,
    ) -> OutputVideo {
        match self {
            Self::Mp4 {
                video_codec,
                audio_codec,
            } => {
                let (audio_codec, transcode_audio) = mp4_audio(
                    allow_audio,
                    audio_codec.is_some(),
                    prescribed_audio_codec,
                    audio_codec,
                );

                OutputVideo {
                    format: OutputVideoFormat::Mp4 {
                        video_codec: prescribed_codec,
                        audio_codec,
                    },
                    transcode_video: !video_codec.const_eq(prescribed_codec),
                    transcode_audio,
                }
            }
            Self::Webm { audio_codec, .. } => {
                let (audio_codec, transcode_audio) = mp4_audio(
                    allow_audio,
                    audio_codec.is_some(),
                    prescribed_audio_codec,
                    None,
                );

                OutputVideo {
                    format: OutputVideoFormat::Mp4 {
                        video_codec: prescribed_codec,
                        audio_codec,
                    },
                    transcode_video: true,
                    transcode_audio,
                }
            }
        }
    }

    pub(crate) const fn build_output(
        self,
        prescribed_video_codec: Option<VideoCodec>,
        prescribed_audio_codec: Option<AudioCodec>,
        allow_audio: bool,
    ) -> OutputVideo {
        match prescribed_video_codec {
            Some(VideoCodec::Vp8) => {
                self.transcode_vorbis(WebmAlphaCodec::Vp8, prescribed_audio_codec, allow_audio)
            }
            Some(VideoCodec::Vp9) => {
                self.transcode_vorbis(WebmAlphaCodec::Vp9, prescribed_audio_codec, allow_audio)
            }
            Some(VideoCodec::Av1) => self.transcode_av1(prescribed_audio_codec, allow_audio),
            Some(VideoCodec::H264) => {
                self.transcode_mp4(Mp4Codec::H264, prescribed_audio_codec, allow_audio)
            }
            Some(VideoCodec::H265) => {
                self.transcode_mp4(Mp4Codec::H265, prescribed_audio_codec, allow_audio)
            }
            None => OutputVideo {
                format: self.to_output(),
                transcode_video: false,
                transcode_audio: false,
            },
        }
    }

    const fn to_output(self) -> OutputVideoFormat {
        match self {
            Self::Mp4 {
                video_codec,
                audio_codec,
            } => OutputVideoFormat::Mp4 {
                video_codec,
                audio_codec,
            },
            Self::Webm {
                video_codec,
                audio_codec,
            } => OutputVideoFormat::Webm {
                video_codec,
                audio_codec,
            },
        }
    }

    pub(crate) const fn ffmpeg_format(self) -> &'static str {
        match self {
            Self::Mp4 { .. } => "mp4",
            Self::Webm { .. } => "webm",
        }
    }
}

impl OutputVideoFormat {
    pub(crate) const fn is_vp9(&self) -> bool {
        match self {
            Self::Webm { video_codec, .. } => video_codec.is_vp9(),
            Self::Mp4 { .. } => false,
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
    const fn is_av1(self) -> bool {
        matches!(self, Self::Av1)
    }

    const fn const_eq(self, rhs: Self) -> bool {
        match (self, rhs) {
            (Self::Av1, Self::Av1) | (Self::H264, Self::H264) | (Self::H265, Self::H265) => true,
            (Self::Av1, _) | (Self::H264, _) | (Self::H265, _) => false,
        }
    }

    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Av1 => "av1",
            Self::H264 => "h264",
            Self::H265 => "hevc",
        }
    }
}

impl AlphaCodec {
    const fn const_eq(self, rhs: Self) -> bool {
        self.alpha == rhs.alpha && self.codec.const_eq(rhs.codec)
    }
}

impl WebmAlphaCodec {
    const fn is_vp9(&self) -> bool {
        matches!(self, Self::Vp9)
    }

    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp9",
        }
    }

    const fn const_eq(self, rhs: Self) -> bool {
        match (self, rhs) {
            (Self::Vp8, Self::Vp8) | (Self::Vp9, Self::Vp9) => true,
            (Self::Vp8, _) | (Self::Vp9, _) => false,
        }
    }
}

impl WebmCodec {
    const fn const_eq(self, rhs: Self) -> bool {
        match (self, rhs) {
            (Self::Av1, Self::Av1) => true,
            (Self::Alpha(this), Self::Alpha(rhs)) => this.const_eq(rhs),
            (Self::Av1, _) | (Self::Alpha(_), _) => false,
        }
    }

    const fn is_vp9(self) -> bool {
        match self {
            Self::Av1 => false,
            Self::Alpha(AlphaCodec { codec, .. }) => codec.is_vp9(),
        }
    }

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
