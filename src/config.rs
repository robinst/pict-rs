use crate::serde_str::Serde;
use clap::{ArgEnum, Parser, Subcommand};
use std::{collections::HashSet, net::SocketAddr, path::PathBuf};
use url::Url;

use crate::magick::ValidInputType;

#[derive(Clone, Debug, Parser)]
pub(crate) struct Args {
    #[clap(short, long, help = "Path to the pict-rs configuration file")]
    config_file: Option<PathBuf>,

    #[clap(subcommand)]
    command: Command,

    #[clap(flatten)]
    overrides: Overrides,
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Clone, Debug, serde::Serialize, Parser)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Overrides {
    #[clap(
        short,
        long,
        help = "Whether to skip validating images uploaded via the internal import API"
    )]
    #[serde(skip_serializing_if = "is_false")]
    skip_validate_imports: bool,

    #[clap(short, long, help = "The address and port the server binds to.")]
    #[serde(skip_serializing_if = "Option::is_none")]
    addr: Option<SocketAddr>,

    #[clap(short, long, help = "The path to the data directory, e.g. data/")]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,

    #[clap(
        short,
        long,
        help = "An optional image format to convert all uploaded files into, supports 'jpg', 'png', and 'webp'"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    image_format: Option<Format>,

    #[clap(
        short,
        long,
        help = "An optional list of filters to permit, supports 'identity', 'thumbnail', 'resize', 'crop', and 'blur'"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<Vec<String>>,

    #[clap(
        short,
        long,
        help = "Specify the maximum allowed uploaded file size (in Megabytes)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,

    #[clap(long, help = "Specify the maximum width in pixels allowed on an image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_image_width: Option<usize>,

    #[clap(long, help = "Specify the maximum width in pixels allowed on an image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_image_height: Option<usize>,

    #[clap(long, help = "Specify the maximum area in pixels allowed in an image")]
    #[serde(skip_serializing_if = "Option::is_none")]
    max_image_area: Option<usize>,

    #[clap(
        long,
        help = "Specify the number of events the console subscriber is allowed to buffer"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    console_buffer_capacity: Option<usize>,

    #[clap(
        long,
        help = "An optional string to be checked on requests to privileged endpoints"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,

    #[clap(
        short,
        long,
        help = "Enable OpenTelemetry Tracing exports to the given OpenTelemetry collector"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    opentelemetry_url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[clap(
        short = 'R',
        long,
        help = "Set the database implementation. Available options are 'sled'. Default is 'sled'"
    )]
    repo: Option<Repo>,

    #[clap(flatten)]
    sled: Sled,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[clap(
        short = 'S',
        long,
        help = "Set the image store. Available options are 'object-storage' or 'filesystem'. Default is 'filesystem'"
    )]
    store: Option<Store>,

    #[clap(flatten)]
    filesystem_storage: FilesystemStorage,

    #[clap(flatten)]
    object_storage: ObjectStorage,
}

impl ObjectStorage {
    pub(crate) fn required(&self) -> Result<RequiredObjectStorage, RequiredError> {
        Ok(RequiredObjectStorage {
            bucket_name: self
                .object_store_bucket_name
                .as_ref()
                .cloned()
                .ok_or(RequiredError("object-store-bucket-name"))?,
            region: self
                .object_store_region
                .as_ref()
                .cloned()
                .map(Serde::into_inner)
                .ok_or(RequiredError("object-store-region"))?,
            access_key: self.object_store_access_key.as_ref().cloned(),
            secret_key: self.object_store_secret_key.as_ref().cloned(),
            security_token: self.object_store_security_token.as_ref().cloned(),
            session_token: self.object_store_session_token.as_ref().cloned(),
        })
    }
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
            && self.console_buffer_capacity.is_none()
            && self.api_key.is_none()
            && self.opentelemetry_url.is_none()
            && self.repo.is_none()
            && self.store.is_none()
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, Subcommand)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub(crate) enum Command {
    Run,
    MigrateStore { to: Store },
    MigrateRepo { to: Repo },
}

pub(crate) enum CommandConfig {
    Run,
    MigrateStore {
        to: Storage,
    },
    MigrateRepo {
        #[allow(dead_code)]
        to: Repository,
    },
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, ArgEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Repo {
    Sled,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize, Parser)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Sled {
    // defaults to {config.path}
    #[clap(long, help = "Path in which pict-rs will create it's 'repo' directory")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sled_path: Option<PathBuf>,

    #[clap(
        long,
        help = "The number of bytes sled is allowed to use for it's in-memory cache"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sled_cache_capacity: Option<u64>,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, ArgEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Store {
    Filesystem,
    ObjectStorage,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize, Parser)]
#[serde(rename_all = "snake_case")]
pub(crate) struct FilesystemStorage {
    // defaults to {config.path}
    #[clap(
        long,
        help = "Path in which pict-rs will create it's 'files' directory"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) filesystem_storage_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize, Parser)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ObjectStorage {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[clap(long, help = "Name of the bucket in which pict-rs will store images")]
    object_store_bucket_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[clap(
        long,
        help = "Region in which the bucket exists, can be an http endpoint"
    )]
    object_store_region: Option<Serde<s3::Region>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[clap(long)]
    object_store_access_key: Option<String>,

    #[clap(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    object_store_secret_key: Option<String>,

    #[clap(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    object_store_security_token: Option<String>,

    #[clap(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    object_store_session_token: Option<String>,
}

pub(crate) struct RequiredSledRepo {
    pub(crate) path: PathBuf,
    pub(crate) cache_capacity: u64,
}

pub(crate) struct RequiredObjectStorage {
    pub(crate) bucket_name: String,
    pub(crate) region: s3::Region,
    pub(crate) access_key: Option<String>,
    pub(crate) secret_key: Option<String>,
    pub(crate) security_token: Option<String>,
    pub(crate) session_token: Option<String>,
}

pub(crate) struct RequiredFilesystemStorage {
    pub(crate) path: PathBuf,
}

pub(crate) enum Storage {
    ObjectStorage(RequiredObjectStorage),
    Filesystem(RequiredFilesystemStorage),
}

pub(crate) enum Repository {
    Sled(RequiredSledRepo),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Config {
    command: Command,
    skip_validate_imports: bool,
    addr: SocketAddr,
    path: PathBuf,
    image_format: Option<Format>,
    filters: Option<Vec<String>>,
    max_file_size: usize,
    max_image_width: usize,
    max_image_height: usize,
    max_image_area: usize,
    console_buffer_capacity: Option<usize>,
    api_key: Option<String>,
    opentelemetry_url: Option<Url>,
    repo: Repo,
    sled: Option<Sled>,
    store: Store,
    filesystem_storage: Option<FilesystemStorage>,
    object_storage: Option<ObjectStorage>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Defaults {
    command: Command,
    skip_validate_imports: bool,
    addr: SocketAddr,
    max_file_size: usize,
    max_image_width: usize,
    max_image_height: usize,
    max_image_area: usize,
    repo: Repo,
    sled: SledDefaults,
    store: Store,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct SledDefaults {
    sled_cache_capacity: usize,
}

impl Defaults {
    fn new() -> Self {
        Defaults {
            command: Command::Run,
            skip_validate_imports: false,
            addr: ([0, 0, 0, 0], 8080).into(),
            max_file_size: 40,
            max_image_width: 10_000,
            max_image_height: 10_000,
            max_image_area: 40_000_000,
            repo: Repo::Sled,
            sled: SledDefaults {
                sled_cache_capacity: 1024 * 1024 * 64,
            },
            store: Store::Filesystem,
        }
    }
}

impl Config {
    pub(crate) fn build() -> anyhow::Result<Self> {
        let args = Args::parse();

        let mut base_config =
            config::Config::builder().add_source(config::Config::try_from(&Defaults::new())?);

        if let Some(path) = args.config_file {
            base_config = base_config.add_source(config::File::from(path));
        };

        if !args.overrides.is_default() {
            let merging = config::Config::try_from(&args.overrides)?;

            base_config = base_config.add_source(merging);
        }

        let config: Self = base_config
            .add_source(config::Environment::with_prefix("PICTRS").separator("__"))
            .build()?
            .try_deserialize()?;

        Ok(config)
    }

    pub(crate) fn command(&self) -> anyhow::Result<CommandConfig> {
        Ok(match &self.command {
            Command::Run => CommandConfig::Run,
            Command::MigrateStore { to } => CommandConfig::MigrateStore {
                to: match to {
                    Store::ObjectStorage => Storage::ObjectStorage(
                        self.object_storage
                            .as_ref()
                            .cloned()
                            .unwrap_or_default()
                            .required()?,
                    ),
                    Store::Filesystem => Storage::Filesystem(RequiredFilesystemStorage {
                        path: self
                            .filesystem_storage
                            .as_ref()
                            .and_then(|f| f.filesystem_storage_path.clone())
                            .unwrap_or_else(|| {
                                let mut path = self.path.clone();
                                path.push("files");
                                path
                            }),
                    }),
                },
            },
            Command::MigrateRepo { to } => CommandConfig::MigrateRepo {
                to: match to {
                    Repo::Sled => {
                        let sled = self.sled.as_ref().cloned().unwrap_or_default();

                        Repository::Sled(RequiredSledRepo {
                            path: sled.sled_path.unwrap_or_else(|| {
                                let mut path = self.path.clone();
                                path.push("sled-repo");
                                path
                            }),
                            cache_capacity: sled.sled_cache_capacity.unwrap_or(1024 * 1024 * 64),
                        })
                    }
                },
            },
        })
    }

    pub(crate) fn store(&self) -> anyhow::Result<Storage> {
        Ok(match self.store {
            Store::Filesystem => Storage::Filesystem(RequiredFilesystemStorage {
                path: self
                    .filesystem_storage
                    .as_ref()
                    .and_then(|f| f.filesystem_storage_path.clone())
                    .unwrap_or_else(|| {
                        let mut path = self.path.clone();
                        path.push("files");
                        path
                    }),
            }),
            Store::ObjectStorage => Storage::ObjectStorage(
                self.object_storage
                    .as_ref()
                    .cloned()
                    .unwrap_or_default()
                    .required()?,
            ),
        })
    }

    pub(crate) fn repo(&self) -> Repository {
        match self.repo {
            Repo::Sled => {
                let sled = self.sled.as_ref().cloned().unwrap_or_default();

                Repository::Sled(RequiredSledRepo {
                    path: sled.sled_path.unwrap_or_else(|| {
                        let mut path = self.path.clone();
                        path.push("sled-repo");
                        path
                    }),
                    cache_capacity: sled.sled_cache_capacity.unwrap_or(1024 * 1024 * 64),
                })
            }
        }
    }

    pub(crate) fn bind_address(&self) -> SocketAddr {
        self.addr
    }

    pub(crate) fn data_dir(&self) -> PathBuf {
        self.path.clone()
    }

    pub(crate) fn console_buffer_capacity(&self) -> Option<usize> {
        self.console_buffer_capacity
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

#[derive(Debug, thiserror::Error)]
#[error("Invalid store supplied, {0}")]
pub(crate) struct StoreError(String);

#[derive(Debug, thiserror::Error)]
#[error("Invalid repo supplied, {0}")]
pub(crate) struct RepoError(String);

#[derive(Debug, thiserror::Error)]
#[error("Missing required {0} field")]
pub(crate) struct RequiredError(&'static str);

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, ArgEnum)]
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
        for variant in Self::value_variants() {
            if variant.to_possible_value().unwrap().matches(s, false) {
                return Ok(*variant);
            }
        }
        Err(FormatError(s.into()))
    }
}

impl std::str::FromStr for Store {
    type Err = StoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for variant in Self::value_variants() {
            if variant.to_possible_value().unwrap().matches(s, false) {
                return Ok(*variant);
            }
        }
        Err(StoreError(s.into()))
    }
}

impl std::str::FromStr for Repo {
    type Err = RepoError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for variant in Self::value_variants() {
            if variant.to_possible_value().unwrap().matches(s, false) {
                return Ok(*variant);
            }
        }
        Err(RepoError(s.into()))
    }
}
