use crate::{
    config::primitives::{LogFormat, Targets},
    serde_str::Serde,
};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Defaults {
    server: ServerDefaults,
    client: ClientDefaults,
    upgrade: UpgradeDefaults,
    tracing: TracingDefaults,
    media: MediaDefaults,
    repo: RepoDefaults,
    store: StoreDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ServerDefaults {
    address: SocketAddr,
    read_only: bool,
    danger_dummy_mode: bool,
    max_file_count: u32,
    temporary_directory: PathBuf,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ClientDefaults {
    timeout: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct UpgradeDefaults {
    concurrency: usize,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct TracingDefaults {
    logging: LoggingDefaults,

    console: ConsoleDefaults,

    opentelemetry: OpenTelemetryDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct LoggingDefaults {
    format: LogFormat,
    targets: Serde<Targets>,
    log_spans: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ConsoleDefaults {
    buffer_capacity: usize,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct OpenTelemetryDefaults {
    service_name: String,
    targets: Serde<Targets>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct MediaDefaults {
    external_validation_timeout: u64,
    max_file_size: usize,
    process_timeout: u64,
    filters: Vec<String>,
    retention: RetentionDefaults,
    image: ImageDefaults,
    animation: AnimationDefaults,
    video: VideoDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct RetentionDefaults {
    variants: &'static str,
    proxy: &'static str,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ImageDefaults {
    max_width: u16,
    max_height: u16,
    max_area: u32,
    max_file_size: usize,
    quality: ImageQualityDefaults,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ImageQualityDefaults {}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct AnimationDefaults {
    max_width: u16,
    max_height: u16,
    max_area: u32,
    max_frame_count: u32,
    max_file_size: usize,
    quality: AnimationQualityDefaults,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct AnimationQualityDefaults {}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct VideoDefaults {
    enable: bool,
    allow_audio: bool,
    max_height: u16,
    max_width: u16,
    max_area: u32,
    max_frame_count: u32,
    max_file_size: usize,
    quality: VideoQualityDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct VideoQualityDefaults {
    crf_max: u8,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct RepoDefaults {
    #[serde(rename = "type")]
    type_: String,

    #[serde(flatten)]
    sled_defaults: SledDefaults,

    #[serde(flatten)]
    postgres_defaults: PostgresDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct SledDefaults {
    path: PathBuf,
    cache_capacity: u64,
    export_path: PathBuf,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct PostgresDefaults {
    use_tls: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct StoreDefaults {
    #[serde(rename = "type")]
    type_: String,

    #[serde(flatten)]
    pub(super) filesystem: FilesystemDefaults,

    #[serde(flatten)]
    pub(super) object_storage: ObjectStorageDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct FilesystemDefaults {
    path: PathBuf,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct ObjectStorageDefaults {
    signature_duration: u64,

    client_timeout: u64,
}

impl Default for ServerDefaults {
    fn default() -> Self {
        Self {
            address: "0.0.0.0:8080".parse().expect("Valid address string"),
            read_only: false,
            danger_dummy_mode: false,
            max_file_count: 1,
            temporary_directory: std::env::temp_dir(),
        }
    }
}

impl Default for ClientDefaults {
    fn default() -> Self {
        Self { timeout: 30 }
    }
}

impl Default for UpgradeDefaults {
    fn default() -> Self {
        Self { concurrency: 32 }
    }
}

impl Default for LoggingDefaults {
    fn default() -> Self {
        Self {
            format: LogFormat::Normal,
            targets: "info".parse().expect("Valid targets string"),
            log_spans: false,
        }
    }
}

impl Default for ConsoleDefaults {
    fn default() -> Self {
        Self {
            buffer_capacity: 1024 * 100,
        }
    }
}

impl Default for OpenTelemetryDefaults {
    fn default() -> Self {
        Self {
            service_name: String::from("pict-rs"),
            targets: "info".parse().expect("Valid targets string"),
        }
    }
}

impl Default for MediaDefaults {
    fn default() -> Self {
        Self {
            external_validation_timeout: 30,
            max_file_size: 40,
            process_timeout: 30,
            filters: vec![
                "blur".into(),
                "crop".into(),
                "identity".into(),
                "resize".into(),
                "thumbnail".into(),
            ],
            retention: Default::default(),
            image: Default::default(),
            animation: Default::default(),
            video: Default::default(),
        }
    }
}

impl Default for RetentionDefaults {
    fn default() -> Self {
        Self {
            variants: "7d",
            proxy: "7d",
        }
    }
}

impl Default for ImageDefaults {
    fn default() -> Self {
        Self {
            max_width: 10_000,
            max_height: 10_000,
            max_area: 40_000_000,
            max_file_size: 40,
            quality: ImageQualityDefaults {},
        }
    }
}

impl Default for AnimationDefaults {
    fn default() -> Self {
        Self {
            max_height: 1920,
            max_width: 1920,
            max_area: 2_073_600,
            max_frame_count: 900,
            max_file_size: 40,
            quality: AnimationQualityDefaults {},
        }
    }
}

impl Default for VideoDefaults {
    fn default() -> Self {
        Self {
            enable: true,
            allow_audio: false,
            max_height: 3_840,
            max_width: 3_840,
            max_area: 8_294_400,
            max_frame_count: 900,
            max_file_size: 40,
            quality: VideoQualityDefaults::default(),
        }
    }
}

impl Default for VideoQualityDefaults {
    fn default() -> Self {
        Self { crf_max: 32 }
    }
}

impl Default for RepoDefaults {
    fn default() -> Self {
        Self {
            type_: String::from("sled"),
            sled_defaults: SledDefaults::default(),
            postgres_defaults: PostgresDefaults::default(),
        }
    }
}

impl Default for SledDefaults {
    fn default() -> Self {
        Self {
            path: PathBuf::from(String::from("/mnt/sled-repo")),
            cache_capacity: 1024 * 1024 * 64,
            export_path: PathBuf::from(String::from("/mnt/exports")),
        }
    }
}

impl Default for StoreDefaults {
    fn default() -> Self {
        Self {
            type_: String::from("filesystem"),
            filesystem: FilesystemDefaults::default(),
            object_storage: ObjectStorageDefaults::default(),
        }
    }
}

impl Default for FilesystemDefaults {
    fn default() -> Self {
        Self {
            path: PathBuf::from(String::from("/mnt/files")),
        }
    }
}

impl Default for ObjectStorageDefaults {
    fn default() -> Self {
        Self {
            signature_duration: 15,
            client_timeout: 30,
        }
    }
}

impl From<crate::config::commandline::Filesystem> for crate::config::primitives::Filesystem {
    fn from(value: crate::config::commandline::Filesystem) -> Self {
        Self {
            path: value
                .path
                .unwrap_or_else(|| FilesystemDefaults::default().path),
        }
    }
}

impl From<crate::config::commandline::Filesystem> for crate::config::primitives::Store {
    fn from(value: crate::config::commandline::Filesystem) -> Self {
        Self::Filesystem(value.into())
    }
}

impl From<SledDefaults> for crate::config::file::Sled {
    fn from(defaults: SledDefaults) -> Self {
        Self {
            path: defaults.path,
            cache_capacity: defaults.cache_capacity,
            export_path: defaults.export_path,
        }
    }
}

impl From<crate::config::commandline::Sled> for crate::config::file::Sled {
    fn from(value: crate::config::commandline::Sled) -> Self {
        let defaults = SledDefaults::default();

        Self {
            path: value.path.unwrap_or(defaults.path),
            cache_capacity: value.cache_capacity.unwrap_or(defaults.cache_capacity),
            export_path: defaults.export_path,
        }
    }
}

impl From<crate::config::commandline::Postgres> for crate::config::file::Postgres {
    fn from(value: crate::config::commandline::Postgres) -> Self {
        Self {
            url: value.url,
            use_tls: value.use_tls,
            certificate_file: value.certificate_file,
        }
    }
}

impl From<crate::config::commandline::Sled> for crate::config::file::Repo {
    fn from(value: crate::config::commandline::Sled) -> Self {
        Self::Sled(value.into())
    }
}

impl From<crate::config::commandline::Postgres> for crate::config::file::Repo {
    fn from(value: crate::config::commandline::Postgres) -> Self {
        Self::Postgres(value.into())
    }
}
