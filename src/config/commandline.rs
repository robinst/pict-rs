use crate::config::primitives::{ImageFormat, LogFormat, Targets};
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use url::Url;

/// Run the pict-rs application
#[derive(Debug, Parser)]
#[clap(author, version, about, long_about = None)]
pub(crate) struct Args {
    /// Path to the pict-rs configuration file
    #[clap(short, long)]
    pub(crate) config_file: Option<PathBuf>,

    /// Format of logs printed to stdout
    #[clap(long)]
    pub(crate) log_format: Option<LogFormat>,
    /// Log levels to print to stdout, respects RUST_LOG formatting
    #[clap(long)]
    pub(crate) log_targets: Option<Targets>,

    /// Address and port to expose tokio-console metrics
    #[clap(long)]
    pub(crate) console_address: Option<SocketAddr>,
    /// Capacity of the console-subscriber Event Buffer
    #[clap(long)]
    pub(crate) console_buffer_capacity: Option<usize>,

    /// URL to send OpenTelemetry metrics
    #[clap(long)]
    pub(crate) opentelemetry_url: Option<Url>,
    /// Service Name to use for OpenTelemetry
    #[clap(long)]
    pub(crate) opentelemetry_service_name: Option<String>,
    /// Log levels to use for OpenTelemetry, respects RUST_LOG formatting
    #[clap(long)]
    pub(crate) opentelemetry_targets: Option<Targets>,

    /// File to save the current configuration for reproducible runs
    #[clap(long)]
    pub(crate) save_to: Option<PathBuf>,

    #[clap(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Runs the pict-rs web server
    Run(Run),

    /// Migrates from one provided media store to another
    #[clap(flatten)]
    MigrateStore(MigrateStore),
}

#[derive(Debug, Parser)]
pub(crate) struct Run {
    /// The address and port to bind the pict-rs web server
    #[clap(short, long)]
    pub(crate) address: SocketAddr,

    /// The API KEY required to access restricted routes
    #[clap(long)]
    pub(crate) api_key: Option<String>,

    /// Whether to validate media on the "import" endpoint
    #[clap(long)]
    pub(crate) media_skip_validate_imports: Option<bool>,
    /// The maximum width, in pixels, for uploaded media
    #[clap(long)]
    pub(crate) media_max_width: Option<usize>,
    /// The maximum height, in pixels, for uploaded media
    #[clap(long)]
    pub(crate) media_max_height: Option<usize>,
    /// The maximum area, in pixels, for uploaded media
    #[clap(long)]
    pub(crate) media_max_area: Option<usize>,
    /// The maximum size, in megabytes, for uploaded media
    #[clap(long)]
    pub(crate) media_max_file_size: Option<usize>,
    /// Whether to enable GIF and silent MP4 uploads. Full videos are unsupported
    #[clap(long)]
    pub(crate) media_enable_silent_video: Option<bool>,
    /// Which media filters should be enabled on the `process` endpoint
    #[clap(long)]
    pub(crate) media_filters: Option<Vec<String>>,
    /// Enforce uploaded media is transcoded to the provided format
    #[clap(long)]
    pub(crate) media_format: Option<ImageFormat>,

    #[clap(subcommand)]
    pub(crate) store: Option<RunStore>,
}

/// Configure the provided storage
#[derive(Debug, Subcommand)]
pub(crate) enum Store {
    /// configure filesystem storage
    Filesystem(Filesystem),

    /// configure object storage
    ObjectStorage(ObjectStorage),
}

/// Run pict-rs with the provided storage
#[derive(Debug, Subcommand)]
pub(crate) enum RunStore {
    /// Run pict-rs with filesystem storage
    Filesystem(RunFilesystem),

    /// Run pict-rs with object storage
    ObjectStorage(RunObjectStorage),
}

/// Configure the pict-rs storage migration
#[derive(Debug, Subcommand)]
pub(crate) enum MigrateStore {
    /// Migrate from the provided filesystem storage
    Filesystem(MigrateFilesystem),

    /// Migrate from the provided object storage
    ObjectStorage(MigrateObjectStorage),
}

/// Migrate pict-rs' storage from the provided filesystem storage
#[derive(Debug, Parser)]
pub(crate) struct MigrateFilesystem {
    #[clap(flatten)]
    pub(crate) from: Filesystem,

    #[clap(subcommand)]
    pub(crate) to: RunStore,
}

/// Migrate pict-rs' storage from the provided object storage
#[derive(Debug, Parser)]
pub(crate) struct MigrateObjectStorage {
    #[clap(flatten)]
    pub(crate) from: ObjectStorage,

    #[clap(subcommand)]
    pub(crate) to: RunStore,
}

/// Run pict-rs with the provided filesystem storage
#[derive(Debug, Parser)]
pub(crate) struct RunFilesystem {
    #[clap(flatten)]
    pub(crate) system: Filesystem,

    #[clap(subcommand)]
    pub(crate) repo: Repo,
}

/// Run pict-rs with the provided object storage
#[derive(Debug, Parser)]
pub(crate) struct RunObjectStorage {
    #[clap(flatten)]
    pub(crate) storage: ObjectStorage,

    #[clap(subcommand)]
    pub(crate) repo: Repo,
}

/// Configuration for data repositories
#[derive(Debug, Subcommand)]
pub(crate) enum Repo {
    /// Run pict-rs with the provided sled-backed data repository
    Sled(Sled),
}

/// Configuration for filesystem media storage
#[derive(Debug, Parser)]
pub(crate) struct Filesystem {
    /// The path to store uploaded media
    #[clap(short, long)]
    pub(crate) path: Option<PathBuf>,
}

/// Configuration for Object Storage
#[derive(Debug, Parser)]
pub(crate) struct ObjectStorage {
    /// The bucket in which to store media
    #[clap(short, long)]
    pub(crate) bucket_name: Option<String>,

    /// The region the bucket is located in
    #[clap(short, long)]
    pub(crate) region: Option<s3::Region>,

    /// The Access Key for the user accessing the bucket
    #[clap(short, long)]
    pub(crate) access_key: Option<String>,

    /// The secret key for the user accessing the bucket
    #[clap(short, long)]
    pub(crate) secret_key: Option<String>,

    /// The security token for accessing the bucket
    #[clap(long)]
    pub(crate) security_token: Option<String>,

    /// The session token for accessing the bucket
    #[clap(long)]
    pub(crate) session_token: Option<String>,
}

/// Configuration for the sled-backed data repository
#[derive(Debug, Parser)]
pub(crate) struct Sled {
    /// The path to store the sled database
    pub(crate) path: Option<PathBuf>,

    /// The cache capacity, in bytes, allowed to sled for in-memory operations
    pub(crate) cache_capacity: Option<u64>,
}
