use crate::{
    config::primitives::{AudioCodec, ImageFormat, LogFormat, Targets, VideoCodec},
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
                client_pool_size,
                media_preprocess_steps,
                media_skip_validate_imports,
                media_max_width,
                media_max_height,
                media_max_area,
                media_max_file_size,
                media_max_frame_count,
                media_gif_max_width,
                media_gif_max_height,
                media_gif_max_area,
                media_enable_silent_video,
                media_enable_full_video,
                media_video_codec,
                media_audio_codec,
                media_filters,
                media_format,
                store,
            }) => {
                let server = Server {
                    address,
                    api_key,
                    worker_id,
                    client_pool_size,
                };
                let gif = if media_gif_max_width.is_none()
                    && media_gif_max_height.is_none()
                    && media_gif_max_area.is_none()
                {
                    None
                } else {
                    Some(Gif {
                        max_width: media_gif_max_width,
                        max_height: media_gif_max_height,
                        max_area: media_gif_max_area,
                    })
                };
                let media = Media {
                    preprocess_steps: media_preprocess_steps,
                    skip_validate_imports: media_skip_validate_imports,
                    max_width: media_max_width,
                    max_height: media_max_height,
                    max_area: media_max_area,
                    max_file_size: media_max_file_size,
                    max_frame_count: media_max_frame_count,
                    gif,
                    enable_silent_video: media_enable_silent_video,
                    enable_full_video: media_enable_full_video,
                    video_codec: media_video_codec,
                    audio_codec: media_audio_codec,
                    filters: media_filters,
                    format: media_format,
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
            Command::MigrateStore(MigrateStore {
                skip_missing_files,
                store,
            }) => {
                let server = Server::default();
                let media = Media::default();

                match store {
                    MigrateStoreFrom::Filesystem(MigrateFilesystem { from, to }) => match to {
                        MigrateStoreTo::Filesystem(MigrateFilesystemInner { to, repo }) => Output {
                            config_format: ConfigFormat {
                                server,
                                old_db,
                                tracing,
                                media,
                                store: None,
                                repo,
                            },
                            operation: Operation::MigrateStore {
                                skip_missing_files,
                                from: from.into(),
                                to: to.into(),
                            },
                            config_file,
                            save_to,
                        },
                        MigrateStoreTo::ObjectStorage(MigrateObjectStorageInner { to, repo }) => {
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
                                    skip_missing_files,
                                    from: from.into(),
                                    to: to.into(),
                                },
                                config_file,
                                save_to,
                            }
                        }
                    },
                    MigrateStoreFrom::ObjectStorage(MigrateObjectStorage { from, to }) => {
                        match to {
                            MigrateStoreTo::Filesystem(MigrateFilesystemInner { to, repo }) => {
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
                                        skip_missing_files,
                                        from: from.into(),
                                        to: to.into(),
                                    },
                                    config_file,
                                    save_to,
                                }
                            }
                            MigrateStoreTo::ObjectStorage(MigrateObjectStorageInner {
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
                                    skip_missing_files,
                                    from: from.into(),
                                    to: to.into(),
                                },
                                config_file,
                                save_to,
                            },
                        }
                    }
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
        skip_missing_files: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    client_pool_size: Option<usize>,
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
    preprocess_steps: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_width: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_height: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_area: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_frame_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gif: Option<Gif>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_silent_video: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_full_video: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_codec: Option<VideoCodec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_codec: Option<AudioCodec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<ImageFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_validate_imports: Option<bool>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Gif {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_width: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_height: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_area: Option<usize>,
}

/// Run the pict-rs application
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub(super) struct Args {
    /// Path to the pict-rs configuration file
    #[arg(short, long)]
    config_file: Option<PathBuf>,

    /// Path to the old pict-rs sled database
    #[arg(long)]
    old_db_path: Option<PathBuf>,

    /// Format of logs printed to stdout
    #[arg(long)]
    log_format: Option<LogFormat>,
    /// Log levels to print to stdout, respects RUST_LOG formatting
    #[arg(long)]
    log_targets: Option<Targets>,

    /// Address and port to expose tokio-console metrics
    #[arg(long)]
    console_address: Option<SocketAddr>,
    /// Capacity of the console-subscriber Event Buffer
    #[arg(long)]
    console_buffer_capacity: Option<usize>,

    /// URL to send OpenTelemetry metrics
    #[arg(long)]
    opentelemetry_url: Option<Url>,
    /// Service Name to use for OpenTelemetry
    #[arg(long)]
    opentelemetry_service_name: Option<String>,
    /// Log levels to use for OpenTelemetry, respects RUST_LOG formatting
    #[arg(long)]
    opentelemetry_targets: Option<Targets>,

    /// File to save the current configuration for reproducible runs
    #[arg(long)]
    save_to: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Runs the pict-rs web server
    Run(Run),

    /// Migrates from one provided media store to another
    MigrateStore(MigrateStore),
}

#[derive(Debug, Parser)]
struct Run {
    /// The address and port to bind the pict-rs web server
    #[arg(short, long)]
    address: Option<SocketAddr>,

    /// The API KEY required to access restricted routes
    #[arg(long)]
    api_key: Option<String>,

    /// ID of this pict-rs node. Doesn't do much yet
    #[arg(long)]
    worker_id: Option<String>,

    /// Number of connections the internel HTTP client should maintain in its pool
    ///
    /// This number defaults to 100, and the total number is multiplied by the number of cores
    /// available to the program. This means that running on a 2 core system will result in 200
    /// pooled connections, and running on a 32 core system will result in 3200 pooled connections.
    #[arg(long)]
    client_pool_size: Option<usize>,

    /// Optional pre-processing steps for uploaded media.
    ///
    /// All still images will be put through these steps before saving
    #[arg(long)]
    media_preprocess_steps: Option<String>,

    /// Whether to validate media on the "import" endpoint
    #[arg(long)]
    media_skip_validate_imports: Option<bool>,
    /// The maximum width, in pixels, for uploaded media
    #[arg(long)]
    media_max_width: Option<usize>,
    /// The maximum height, in pixels, for uploaded media
    #[arg(long)]
    media_max_height: Option<usize>,
    /// The maximum area, in pixels, for uploaded media
    #[arg(long)]
    media_max_area: Option<usize>,
    /// The maximum size, in megabytes, for uploaded media
    #[arg(long)]
    media_max_file_size: Option<usize>,
    /// The maximum number of frames allowed for uploaded GIF and MP4s.
    #[arg(long)]
    media_max_frame_count: Option<usize>,
    /// Maximum width allowed for gif uploads.
    ///
    /// If an upload exceeds this value, it will be transcoded to a video format or rejected,
    /// depending on whether video uploads are enabled.
    #[arg(long)]
    media_gif_max_width: Option<usize>,
    /// Maximum height allowed for gif uploads
    ///
    /// If an upload exceeds this value, it will be transcoded to a video format or rejected,
    /// depending on whether video uploads are enabled.
    #[arg(long)]
    media_gif_max_height: Option<usize>,
    /// Maximum area allowed for gif uploads
    ///
    /// If an upload exceeds this value, it will be transcoded to a video format or rejected,
    /// depending on whether video uploads are enabled.
    #[arg(long)]
    media_gif_max_area: Option<usize>,
    /// Whether to enable GIF and silent video uploads
    #[arg(long)]
    media_enable_silent_video: Option<bool>,
    /// Whether to enable full video uploads
    #[arg(long)]
    media_enable_full_video: Option<bool>,
    /// Enforce a specific video codec for uploaded videos
    #[arg(long)]
    media_video_codec: Option<VideoCodec>,
    /// Enforce a specific audio codec for uploaded videos
    #[arg(long)]
    media_audio_codec: Option<AudioCodec>,
    /// Which media filters should be enabled on the `process` endpoint
    #[arg(long)]
    media_filters: Option<Vec<String>>,
    /// Enforce uploaded media is transcoded to the provided format
    #[arg(long)]
    media_format: Option<ImageFormat>,

    #[command(subcommand)]
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

#[derive(Debug, Parser)]
struct MigrateStore {
    /// Normally, pict-rs will keep retrying when errors occur during migration. This flag tells
    /// pict-rs to ignore errors that are caused by files not existing.
    #[arg(long)]
    skip_missing_files: bool,

    #[command(subcommand)]
    store: MigrateStoreFrom,
}

/// Configure the pict-rs storage migration
#[derive(Debug, Subcommand)]
enum MigrateStoreFrom {
    /// Migrate from the provided filesystem storage
    Filesystem(MigrateFilesystem),

    /// Migrate from the provided object storage
    ObjectStorage(MigrateObjectStorage),
}

/// Configure the destination storage for pict-rs storage migration
#[derive(Debug, Subcommand)]
enum MigrateStoreTo {
    /// Migrate to the provided filesystem storage
    Filesystem(MigrateFilesystemInner),

    /// Migrate to the provided object storage
    ObjectStorage(MigrateObjectStorageInner),
}

/// Migrate pict-rs' storage from the provided filesystem storage
#[derive(Debug, Parser)]
struct MigrateFilesystem {
    #[command(flatten)]
    from: Filesystem,

    #[command(subcommand)]
    to: MigrateStoreTo,
}

/// Migrate pict-rs' storage to the provided filesystem storage
#[derive(Debug, Parser)]
struct MigrateFilesystemInner {
    #[command(flatten)]
    to: Filesystem,

    #[command(subcommand)]
    repo: Option<Repo>,
}

/// Migrate pict-rs' storage from the provided object storage
#[derive(Debug, Parser)]
struct MigrateObjectStorage {
    #[command(flatten)]
    from: crate::config::primitives::ObjectStorage,

    #[command(subcommand)]
    to: MigrateStoreTo,
}

/// Migrate pict-rs' storage to the provided object storage
#[derive(Debug, Parser)]
struct MigrateObjectStorageInner {
    #[command(flatten)]
    to: crate::config::primitives::ObjectStorage,

    #[command(subcommand)]
    repo: Option<Repo>,
}

/// Run pict-rs with the provided filesystem storage
#[derive(Debug, Parser)]
struct RunFilesystem {
    #[command(flatten)]
    system: Filesystem,

    #[command(subcommand)]
    repo: Option<Repo>,
}

/// Run pict-rs with the provided object storage
#[derive(Debug, Parser)]
struct RunObjectStorage {
    #[command(flatten)]
    storage: ObjectStorage,

    #[command(subcommand)]
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
pub(super) struct Filesystem {
    /// The path to store uploaded media
    #[arg(short, long)]
    pub(super) path: Option<PathBuf>,
}

/// Configuration for Object Storage
#[derive(Clone, Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ObjectStorage {
    /// The base endpoint for the object storage
    ///
    /// Examples:
    /// - `http://localhost:9000`
    /// - `https://s3.dualstack.eu-west-1.amazonaws.com`
    #[arg(short, long)]
    endpoint: Url,

    /// Determines whether to use path style or virtualhost style for accessing objects
    ///
    /// When this is true, objects will be fetched from {endpoint}/{bucket_name}/{object}
    /// When false, objects will be fetched from {bucket_name}.{endpoint}/{object}
    #[arg(short, long)]
    use_path_style: bool,

    /// The bucket in which to store media
    #[arg(short, long)]
    bucket_name: Option<String>,

    /// The region the bucket is located in
    ///
    /// For minio deployments, this can just be 'minio'
    #[arg(short, long)]
    region: Option<String>,

    /// The Access Key for the user accessing the bucket
    #[arg(short, long)]
    access_key: Option<String>,

    /// The secret key for the user accessing the bucket
    #[arg(short, long)]
    secret_key: Option<String>,

    /// The session token for accessing the bucket
    #[arg(long)]
    session_token: Option<String>,
}

/// Configuration for the sled-backed data repository
#[derive(Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Sled {
    /// The path to store the sled database
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,

    /// The cache capacity, in bytes, allowed to sled for in-memory operations
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_capacity: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    export_path: Option<PathBuf>,
}
