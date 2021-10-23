use std::{collections::HashSet, net::SocketAddr, path::PathBuf};
use url::Url;

use crate::magick::ValidInputType;

#[derive(Clone, Debug, structopt::StructOpt)]
pub(crate) struct Config {
    #[structopt(
        short,
        long,
        help = "Whether to skip validating images uploaded via the internal import API"
    )]
    skip_validate_imports: bool,

    #[structopt(
        short,
        long,
        env = "PICTRS_ADDR",
        default_value = "0.0.0.0:8080",
        help = "The address and port the server binds to."
    )]
    addr: SocketAddr,

    #[structopt(
        short,
        long,
        env = "PICTRS_PATH",
        help = "The path to the data directory, e.g. data/"
    )]
    path: PathBuf,

    #[structopt(
        short,
        long,
        env = "PICTRS_FORMAT",
        help = "An optional image format to convert all uploaded files into, supports 'jpg', 'png', and 'webp'"
    )]
    image_format: Option<Format>,

    #[structopt(
        short,
        long,
        env = "PICTRS_ALLOWED_FILTERS",
        help = "An optional list of filters to permit, supports 'identity', 'thumbnail', 'resize', 'crop', and 'blur'"
    )]
    filters: Option<Vec<String>>,

    #[structopt(
        short,
        long,
        env = "PICTRS_MAX_FILE_SIZE",
        help = "Specify the maximum allowed uploaded file size (in Megabytes)",
        default_value = "40"
    )]
    max_file_size: usize,

    #[structopt(
        long,
        env = "PICTRS_MAX_IMAGE_WIDTH",
        help = "Specify the maximum width in pixels allowed on an image",
        default_value = "10000"
    )]
    max_image_width: usize,

    #[structopt(
        long,
        env = "PICTRS_MAX_IMAGE_HEIGHT",
        help = "Specify the maximum width in pixels allowed on an image",
        default_value = "10000"
    )]
    max_image_height: usize,

    #[structopt(
        long,
        env = "PICTRS_API_KEY",
        help = "An optional string to be checked on requests to privileged endpoints"
    )]
    api_key: Option<String>,

    #[structopt(
        short,
        long,
        env = "PICTRS_OPENTELEMETRY_URL",
        help = "Enable OpenTelemetry Tracing exports to the given OpenTelemetry collector"
    )]
    opentelemetry_url: Option<Url>,
}

impl Config {
    pub(crate) fn bind_address(&self) -> SocketAddr {
        self.addr
    }

    pub(crate) fn data_dir(&self) -> PathBuf {
        self.path.clone()
    }

    pub(crate) fn format(&self) -> Option<Format> {
        self.image_format.clone()
    }

    pub(crate) fn allowed_filters(&self) -> Option<HashSet<String>> {
        self.filters.as_ref().map(|wl| wl.iter().cloned().collect())
    }

    pub(crate) fn validate_imports(&self) -> bool {
        !self.skip_validate_imports
    }

    pub(crate) fn max_file_size(&self) -> usize {
        self.max_file_size
    }

    pub(crate) fn max_width(&self) -> usize {
        self.max_image_width
    }

    pub(crate) fn max_height(&self) -> usize {
        self.max_image_height
    }

    pub(crate) fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub(crate) fn opentelemetry_url(&self) -> Option<&Url> {
        self.opentelemetry_url.as_ref()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid format supplied, {0}")]
pub(crate) struct FormatError(String);

#[derive(Copy, Clone, Debug)]
pub(crate) enum Format {
    Jpeg,
    Png,
    Webp,
}

impl Format {
    pub(crate) fn to_magick_format(&self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::Webp => "WEBP",
        }
    }

    pub(crate) fn to_hint(&self) -> Option<ValidInputType> {
        match self {
            Format::Jpeg => Some(ValidInputType::Jpeg),
            Format::Png => Some(ValidInputType::Png),
            Format::Webp => Some(ValidInputType::Webp),
        }
    }
}

impl std::str::FromStr for Format {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "png" => Ok(Format::Png),
            "jpg" => Ok(Format::Jpeg),
            "webp" => Ok(Format::Webp),
            other => Err(FormatError(other.to_string())),
        }
    }
}
