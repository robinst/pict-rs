#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) struct AnimationInput {
    pub(crate) format: AnimationFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) struct AnimationOutput {
    pub(crate) format: AnimationFormat,
    pub(crate) needs_transcode: bool,
}

impl AnimationInput {
    pub(crate) const fn build_output(
        &self,
        prescribed: Option<AnimationFormat>,
    ) -> AnimationOutput {
        if let Some(prescribed) = prescribed {
            let needs_transcode = !self.format.const_eq(prescribed);

            return AnimationOutput {
                format: prescribed,
                needs_transcode,
            };
        }

        AnimationOutput {
            format: self.format,
            needs_transcode: false,
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

    pub(super) const fn file_extension(self) -> &'static str {
        match self {
            Self::Apng => ".apng",
            Self::Avif => ".avif",
            Self::Gif => ".gif",
            Self::Webp => ".webp",
        }
    }

    pub(crate) const fn magick_format(self) -> &'static str {
        match self {
            Self::Apng => "APNG",
            Self::Avif => "AVIF",
            Self::Gif => "GIF",
            Self::Webp => "WEBP",
        }
    }

    pub(super) fn media_type(self) -> mime::Mime {
        match self {
            Self::Apng => super::mimes::image_apng(),
            Self::Avif => super::mimes::image_avif(),
            Self::Gif => mime::IMAGE_GIF,
            Self::Webp => super::mimes::image_webp(),
        }
    }
}
