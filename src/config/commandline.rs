use crate::{
    config::primitives::{ImageFormat, LogFormat, Targets},
    serde_str::Serde,
};
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use url::Url;

impl Args {
    pub(super) fn into_output(self) -> Output {
        let Args {
            config_file,
            old_db_path,
            log_format,
            log_targets,
            console_address,
            console_buffer_capacity,
            opentelemetry_url,
            opentelemetry_service_name,
            opentelemetry_targets,
            save_to,
            command,
        } = self;

        let old_db = OldDb { path: old_db_path };

        let tracing = Tracing {
            logging: Logging {
                format: log_format,
                targets: log_targets.map(Serde::new),
            },
            console: Console {
                address: console_address,
                buffer_capacity: console_buffer_capacity,
            },
            opentelemetry: OpenTelemetry {
                url: opentelemetry_url,
                service_name: opentelemetry_service_name,
                targets: opentelemetry_targets.map(Serde::new),
            },
        };

        match command {
            Command::Run(Run {
                address,
                api_key,
                worker_id,
                media_skip_validate_imports,
                media_max_width,
                media_max_height,
                media_max_area,
                media_max_file_size,
                media_enable_silent_video,
                media_filters,
                media_format,
                media_cache_duration,
                store,
            }) => {
                let server = Server {
                    address,
                    api_key,
                    worker_id,
                };
                let media = Media {
                    skip_validate_imports: media_skip_validate_imports,
                    max_width: media_max_width,
                    max_height: media_max_height,
                    max_area: media_max_area,
                    max_file_size: media_max_file_size,
                    enable_silent_video: media_enable_silent_video,
                    filters: media_filters,
                    format: media_format,
                    cache_duration: media_cache_duration,
                };
                let operation = Operation::Run;

                match store {
                    Some(RunStore::Filesystem(RunFilesystem { system, repo })) => {
                        let store = Some(Store::Filesystem(system));
                        Output {
                            config_format: ConfigFormat {
                                server,
                                old_db,
                                tracing,
                                media,
                                store,
                                repo,
                            },
                            operation,
                            config_file,
                            save_to,
                        }
                    }
                    Some(RunStore::ObjectStorage(RunObjectStorage { storage, repo })) => {
                        let store = Some(Store::ObjectStorage(storage));
                        Output {
                            config_format: ConfigFormat {
                                server,
                                old_db,
                                tracing,
                                media,
                                store,
                                repo,
                            },
                            operation,
                            config_file,
                            save_to,
                        }
                    }
                    None => Output {
                        config_format: ConfigFormat {
                            server,
                            old_db,
                            tracing,
                            media,
                            store: None,
                            repo: None,
                        },
                        operation,
                        config_file,
                        save_to,
                    },
                }
            }
            Command::MigrateStore(migrate_store) => {
                let server = Server::default();
                let media = Media::default();

                match migrate_store {
                    MigrateStore::Filesystem(MigrateFilesystem { from, to }) => match to {
                        MigrateStoreInner::Filesystem(MigrateFilesystemInner { to, repo }) => {
                            Output {
                                config_format: ConfigFormat {
                                    server,
                                    old_db,
                                    tracing,
                                    media,
                                    store: None,
                                    repo,
                                },
                                operation: Operation::MigrateStore {
                                    from: from.into(),
                                    to: to.into(),
                                },
                                config_file,
                                save_to,
                            }
                        }
                        MigrateStoreInner::ObjectStorage(MigrateObjectStorageInner {
                            to,
                            repo,
                        }) => Output {
                            config_format: ConfigFormat {
                                server,
                                old_db,
                                tracing,
                                media,
                                store: None,
                                repo,
                            },
                            operation: Operation::MigrateStore {
                                from: from.into(),
                                to: to.into(),
                            },
                            config_file,
                            save_to,
                        },
                    },
                    MigrateStore::ObjectStorage(MigrateObjectStorage { from, to }) => match to {
                        MigrateStoreInner::Filesystem(MigrateFilesystemInner { to, repo }) => {
                            Output {
                                config_format: ConfigFormat {
                                    server,
                                    old_db,
                                    tracing,
                                    media,
                                    store: None,
                                    repo,
                                },
                                operation: Operation::MigrateStore {
                                    from: from.into(),
                                    to: to.into(),
                                },
                                config_file,
                                save_to,
                            }
                        }
                        MigrateStoreInner::ObjectStorage(MigrateObjectStorageInner {
                            to,
                            repo,
                        }) => Output {
                            config_format: ConfigFormat {
                                server,
                                old_db,
                                tracing,
                                media,
                                store: None,
                                repo,
                            },
                            operation: Operation::MigrateStore {
                                from: from.into(),
                                to: to.into(),
                            },
                            config_file,
                            save_to,
                        },
                    },
                }
            }
        }
    }
}

pub(super) struct Output {
    pub(super) config_format: ConfigFormat,
    pub(super) operation: Operation,
    pub(super) save_to: Option<PathBuf>,
    pub(super) config_file: Option<PathBuf>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub(crate) enum Operation {
    Run,
    MigrateStore {
        from: crate::config::primitives::Store,
        to: crate::config::primitives::Store,
    },
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct ConfigFormat {
    server: Server,
    old_db: OldDb,
    tracing: Tracing,
    media: Media,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<Repo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<Store>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Server {
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<SocketAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Tracing {
    logging: Logging,
    console: Console,
    opentelemetry: OpenTelemetry,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Logging {
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<LogFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<Serde<Targets>>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Console {
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<SocketAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    buffer_capacity: Option<usize>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct OpenTelemetry {
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<Serde<Targets>>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct OldDb {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Media {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_width: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_height: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_area: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_silent_video: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<ImageFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_validate_imports: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_duration: Option<i64>,
}

/// Run the pict-rs application
#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
pub(super) struct Args {
    /// Path to the pict-rs configuration file
    #[clap(short, long)]
    config_file: Option<PathBuf>,

    /// Path to the old pict-rs sled database
    #[clap(long)]
    old_db_path: Option<PathBuf>,

    /// Format of logs printed to stdout
    #[clap(long)]
    log_format: Option<LogFormat>,
    /// Log levels to print to stdout, respects RUST_LOG formatting
    #[clap(long)]
    log_targets: Option<Targets>,

    /// Address and port to expose tokio-console metrics
    #[clap(long)]
    console_address: Option<SocketAddr>,
    /// Capacity of the console-subscriber Event Buffer
    #[clap(long)]
    console_buffer_capacity: Option<usize>,

    /// URL to send OpenTelemetry metrics
    #[clap(long)]
    opentelemetry_url: Option<Url>,
    /// Service Name to use for OpenTelemetry
    #[clap(long)]
    opentelemetry_service_name: Option<String>,
    /// Log levels to use for OpenTelemetry, respects RUST_LOG formatting
    #[clap(long)]
    opentelemetry_targets: Option<Targets>,

    /// File to save the current configuration for reproducible runs
    #[clap(long)]
    save_to: Option<PathBuf>,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Runs the pict-rs web server
    Run(Run),

    /// Migrates from one provided media store to another
    #[clap(flatten)]
    MigrateStore(MigrateStore),
}

#[derive(Debug, Parser)]
struct Run {
    /// The address and port to bind the pict-rs web server
    #[clap(short, long)]
    address: Option<SocketAddr>,

    /// The API KEY required to access restricted routes
    #[clap(long)]
    api_key: Option<String>,

    #[clap(long)]
    worker_id: Option<String>,

    /// Whether to validate media on the "import" endpoint
    #[clap(long)]
    media_skip_validate_imports: Option<bool>,
    /// The maximum width, in pixels, for uploaded media
    #[clap(long)]
    media_max_width: Option<usize>,
    /// The maximum height, in pixels, for uploaded media
    #[clap(long)]
    media_max_height: Option<usize>,
    /// The maximum area, in pixels, for uploaded media
    #[clap(long)]
    media_max_area: Option<usize>,
    /// The maximum size, in megabytes, for uploaded media
    #[clap(long)]
    media_max_file_size: Option<usize>,
    /// Whether to enable GIF and silent MP4 uploads. Full videos are unsupported
    #[clap(long)]
    media_enable_silent_video: Option<bool>,
    /// Which media filters should be enabled on the `process` endpoint
    #[clap(long)]
    media_filters: Option<Vec<String>>,
    /// Enforce uploaded media is transcoded to the provided format
    #[clap(long)]
    media_format: Option<ImageFormat>,

    /// How long, in hours, to keep media ingested through the "cached" endpoint
    #[clap(long)]
    media_cache_duration: Option<i64>,

    #[clap(subcommand)]
    store: Option<RunStore>,
}

/// Configure the provided storage
#[derive(Clone, Debug, Subcommand, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
enum Store {
    /// configure filesystem storage
    Filesystem(Filesystem),

    /// configure object storage
    ObjectStorage(ObjectStorage),
}

/// Run pict-rs with the provided storage
#[derive(Debug, Subcommand)]
enum RunStore {
    /// Run pict-rs with filesystem storage
    Filesystem(RunFilesystem),

    /// Run pict-rs with object storage
    ObjectStorage(RunObjectStorage),
}

/// Configure the pict-rs storage migration
#[derive(Debug, Subcommand)]
enum MigrateStore {
    /// Migrate from the provided filesystem storage
    Filesystem(MigrateFilesystem),

    /// Migrate from the provided object storage
    ObjectStorage(MigrateObjectStorage),
}

/// Configure the destination storage for pict-rs storage migration
#[derive(Debug, Subcommand)]
enum MigrateStoreInner {
    /// Migrate to the provided filesystem storage
    Filesystem(MigrateFilesystemInner),

    /// Migrate to the provided object storage
    ObjectStorage(MigrateObjectStorageInner),
}

/// Migrate pict-rs' storage from the provided filesystem storage
#[derive(Debug, Parser)]
struct MigrateFilesystem {
    #[clap(flatten)]
    from: crate::config::primitives::Filesystem,

    #[clap(subcommand)]
    to: MigrateStoreInner,
}

/// Migrate pict-rs' storage to the provided filesystem storage
#[derive(Debug, Parser)]
struct MigrateFilesystemInner {
    #[clap(flatten)]
    to: crate::config::primitives::Filesystem,

    #[clap(subcommand)]
    repo: Option<Repo>,
}

/// Migrate pict-rs' storage from the provided object storage
#[derive(Debug, Parser)]
struct MigrateObjectStorage {
    #[clap(flatten)]
    from: crate::config::primitives::ObjectStorage,

    #[clap(subcommand)]
    to: MigrateStoreInner,
}

/// Migrate pict-rs' storage to the provided object storage
#[derive(Debug, Parser)]
struct MigrateObjectStorageInner {
    #[clap(flatten)]
    to: crate::config::primitives::ObjectStorage,

    #[clap(subcommand)]
    repo: Option<Repo>,
}

/// Run pict-rs with the provided filesystem storage
#[derive(Debug, Parser)]
struct RunFilesystem {
    #[clap(flatten)]
    system: Filesystem,

    #[clap(subcommand)]
    repo: Option<Repo>,
}

/// Run pict-rs with the provided object storage
#[derive(Debug, Parser)]
struct RunObjectStorage {
    #[clap(flatten)]
    storage: ObjectStorage,

    #[clap(subcommand)]
    repo: Option<Repo>,
}

/// Configuration for data repositories
#[derive(Debug, Subcommand, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
enum Repo {
    /// Run pict-rs with the provided sled-backed data repository
    Sled(Sled),
}

/// Configuration for filesystem media storage
#[derive(Clone, Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Filesystem {
    /// The path to store uploaded media
    #[clap(short, long)]
    path: Option<PathBuf>,
}

/// Configuration for Object Storage
#[derive(Clone, Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ObjectStorage {
    /// The bucket in which to store media
    #[clap(short, long)]
    bucket_name: Option<String>,

    /// The region the bucket is located in
    #[clap(short, long)]
    region: Option<Serde<s3::Region>>,

    /// The Access Key for the user accessing the bucket
    #[clap(short, long)]
    access_key: Option<String>,

    /// The secret key for the user accessing the bucket
    #[clap(short, long)]
    secret_key: Option<String>,

    /// The session token for accessing the bucket
    #[clap(long)]
    session_token: Option<String>,
}

/// Configuration for the sled-backed data repository
#[derive(Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Sled {
    /// The path to store the sled database
    #[clap(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,

    /// The cache capacity, in bytes, allowed to sled for in-memory operations
    #[clap(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_capacity: Option<u64>,
}
