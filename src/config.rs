use std::{collections::HashSet, net::SocketAddr, path::PathBuf};
use structopt::StructOpt;
use url::Url;

use crate::magick::ValidInputType;

#[derive(Clone, Debug, StructOpt)]
pub(crate) struct Args {
    #[structopt(short, long, help = "Path to the pict-rs configuration file")]
    config_file: Option<PathBuf>,

    #[structopt(flatten)]
    overrides: Overrides,
}

#[derive(Clone, Debug, serde::Serialize, structopt::StructOpt)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Overrides {
    #[structopt(
        short,
        long,
        help = "Whether to skip validating images uploaded via the internal import API"
    )]
    skip_validate_imports: bool,

    #[structopt(short, long, help = "The address and port the server binds to.")]
    addr: Option<SocketAddr>,

    #[structopt(short, long, help = "The path to the data directory, e.g. data/")]
    path: Option<PathBuf>,

    #[structopt(
        short,
        long,
        help = "An optional image format to convert all uploaded files into, supports 'jpg', 'png', and 'webp'"
    )]
    image_format: Option<Format>,

    #[structopt(
        short,
        long,
        help = "An optional list of filters to permit, supports 'identity', 'thumbnail', 'resize', 'crop', and 'blur'"
    )]
    filters: Option<Vec<String>>,

    #[structopt(
        short,
        long,
        help = "Specify the maximum allowed uploaded file size (in Megabytes)"
    )]
    max_file_size: Option<usize>,

    #[structopt(long, help = "Specify the maximum width in pixels allowed on an image")]
    max_image_width: Option<usize>,

    #[structopt(long, help = "Specify the maximum width in pixels allowed on an image")]
    max_image_height: Option<usize>,

    #[structopt(long, help = "Specify the maximum area in pixels allowed in an image")]
    max_image_area: Option<usize>,

    #[structopt(
        long,
        help = "An optional string to be checked on requests to privileged endpoints"
    )]
    api_key: Option<String>,

    #[structopt(
        short,
        long,
        help = "Enable OpenTelemetry Tracing exports to the given OpenTelemetry collector"
    )]
    opentelemetry_url: Option<Url>,

    #[structopt(subcommand)]
    store: Option<Store>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, structopt::StructOpt)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "type")]
pub(crate) enum Store {
    FileStore {
        // defaults to {config.path}
        path: Option<PathBuf>,
    },
    #[cfg(feature = "object-storage")]
    S3Store {
        bucket_name: String,
        region: crate::serde_str::Serde<s3::Region>,
        access_key: Option<String>,
        secret_key: Option<String>,
        security_token: Option<String>,
        session_token: Option<String>,
    },
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Config {
    skip_validate_imports: bool,
    addr: SocketAddr,
    path: PathBuf,
    image_format: Option<Format>,
    filters: Option<Vec<String>>,
    max_file_size: usize,
    max_image_width: usize,
    max_image_height: usize,
    max_image_area: usize,
    api_key: Option<String>,
    opentelemetry_url: Option<Url>,
    store: Store,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Defaults {
    skip_validate_imports: bool,
    addr: SocketAddr,
    max_file_size: usize,
    max_image_width: usize,
    max_image_height: usize,
    max_image_area: usize,
    store: Store,
}

impl Defaults {
    fn new() -> Self {
        Defaults {
            skip_validate_imports: false,
            addr: ([0, 0, 0, 0], 8080).into(),
            max_file_size: 40,
            max_image_width: 10_000,
            max_image_height: 10_000,
            max_image_area: 40_000,
            store: Store::FileStore { path: None },
        }
    }
}

impl Config {
    pub(crate) fn build() -> anyhow::Result<Self> {
        let args = Args::from_args();
        let mut base_config = config::Config::try_from(&Defaults::new())?;

        if let Some(path) = args.config_file {
            base_config.merge(config::File::from(path))?;
        };

        base_config.merge(config::Config::try_from(&args.overrides)?)?;

        base_config.merge(config::Environment::with_prefix("PICTRS"))?;

        Ok(base_config.try_into()?)
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn bind_address(&self) -> SocketAddr {
        self.addr
    }

    pub(crate) fn data_dir(&self) -> PathBuf {
        self.path.clone()
    }

    pub(crate) fn format(&self) -> Option<Format> {
        self.image_format
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

    pub(crate) fn max_area(&self) -> usize {
        self.max_image_area
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

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Format {
    Jpeg,
    Png,
    Webp,
}

impl Format {
    pub(crate) fn as_magick_format(&self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::Webp => "WEBP",
        }
    }

    pub(crate) fn as_hint(&self) -> Option<ValidInputType> {
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
