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
pub(crate) struct ImageInput {
    pub(crate) format: ImageFormat,
    pub(crate) needs_reorient: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
pub(crate) struct ImageOutput {
    pub(crate) format: ImageFormat,
    pub(crate) needs_transcode: bool,
}

impl ImageInput {
    pub(crate) const fn build_output(self, prescribed: Option<ImageFormat>) -> ImageOutput {
        if let Some(prescribed) = prescribed {
            let needs_transcode = self.needs_reorient || !self.format.const_eq(prescribed);

            return ImageOutput {
                format: prescribed,
                needs_transcode,
            };
        }

        ImageOutput {
            format: self.format,
            needs_transcode: self.needs_reorient,
        }
    }
}

impl ImageFormat {
    pub(super) const fn const_eq(self, rhs: Self) -> bool {
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

    pub(crate) const fn file_extension(self) -> &'static str {
        match self {
            Self::Avif => ".avif",
            Self::Jpeg => ".jpeg",
            Self::Jxl => ".jxl",
            Self::Png => ".png",
            Self::Webp => ".webp",
        }
    }

    pub(crate) const fn magick_format(self) -> &'static str {
        match self {
            Self::Avif => "AVIF",
            Self::Jpeg => "JPEG",
            Self::Jxl => "JXL",
            Self::Png => "PNG",
            Self::Webp => "Webp",
        }
    }

    pub(crate) fn media_type(self) -> mime::Mime {
        match self {
            Self::Avif => super::mimes::image_avif(),
            Self::Jpeg => mime::IMAGE_JPEG,
            Self::Jxl => super::mimes::image_jxl(),
            Self::Png => mime::IMAGE_PNG,
            Self::Webp => super::mimes::image_webp(),
        }
    }
}
