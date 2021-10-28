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

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Clone, Debug, serde::Serialize, structopt::StructOpt)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Overrides {
    #[structopt(
        short,
        long,
        help = "Whether to skip validating images uploaded via the internal import API"
    )]
    #[serde(skip_serializing_if = "is_false")]
    skip_validate_imports: bool,

    #[structopt(short, long, help = "The address and port the server binds to.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    addr: Option<SocketAddr>,

    #[structopt(short, long, help = "The path to the data directory, e.g. data/")]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,

    #[structopt(
        short,
        long,
        help = "An optional image format to convert all uploaded files into, supports 'jpg', 'png', and 'webp'"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    image_format: Option<Format>,

    #[structopt(
        short,
        long,
        help = "An optional list of filters to permit, supports 'identity', 'thumbnail', 'resize', 'crop', and 'blur'"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<Vec<String>>,

    #[structopt(
        short,
        long,
        help = "Specify the maximum allowed uploaded file size (in Megabytes)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,

    #[structopt(long, help = "Specify the maximum width in pixels allowed on an image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_image_width: Option<usize>,

    #[structopt(long, help = "Specify the maximum width in pixels allowed on an image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_image_height: Option<usize>,

    #[structopt(long, help = "Specify the maximum area in pixels allowed in an image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_image_area: Option<usize>,

    #[structopt(
        long,
        help = "An optional string to be checked on requests to privileged endpoints"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,

    #[structopt(
        short,
        long,
        help = "Enable OpenTelemetry Tracing exports to the given OpenTelemetry collector"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    opentelemetry_url: Option<Url>,

    #[structopt(subcommand)]
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<Store>,
}

impl Overrides {
    fn is_default(&self) -> bool {
        !self.skip_validate_imports
            && self.addr.is_none()
            && self.path.is_none()
            && self.image_format.is_none()
            && self.filters.is_none()
            && self.max_file_size.is_none()
            && self.max_image_width.is_none()
            && self.max_image_height.is_none()
            && self.max_image_area.is_none()
            && self.api_key.is_none()
            && self.opentelemetry_url.is_none()
            && self.store.is_none()
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, structopt::StructOpt)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub(crate) enum Store {
    FileStore {
        // defaults to {config.path}
        #[structopt(long)]
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
    #[cfg(feature = "object-storage")]
    S3Store {
        #[structopt(long)]
        bucket_name: String,

        #[structopt(long)]
        region: crate::serde_str::Serde<s3::Region>,

        #[serde(skip_serializing_if = "Option::is_none")]
        #[structopt(long)]
        access_key: Option<String>,

        #[structopt(long)]
        #[serde(skip_serializing_if = "Option::is_none")]
        secret_key: Option<String>,

        #[structopt(long)]
        #[serde(skip_serializing_if = "Option::is_none")]
        security_token: Option<String>,

        #[structopt(long)]
        #[serde(skip_serializing_if = "Option::is_none")]
        session_token: Option<String>,
    },
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
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
#[serde(rename_all = "snake_case")]
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
        let mut base_config = config::Config::new();
        base_config.merge(config::Config::try_from(&Defaults::new())?)?;

        if let Some(path) = args.config_file {
            base_config.merge(config::File::from(path))?;
        };

        if !args.overrides.is_default() {
            let merging = config::Config::try_from(&args.overrides)?;

            base_config.merge(merging)?;
        }

        base_config.merge(config::Environment::with_prefix("PICTRS").separator("__"))?;

        let config: Self = base_config.try_into()?;

        Ok(config)
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
#[serde(rename_all = "snake_case")]
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
