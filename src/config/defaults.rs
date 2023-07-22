use crate::{
    config::primitives::{LogFormat, Targets},
    formats::VideoCodec,
    serde_str::Serde,
};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Defaults {
    server: ServerDefaults,
    client: ClientDefaults,
    tracing: TracingDefaults,
    old_db: OldDbDefaults,
    media: MediaDefaults,
    repo: RepoDefaults,
    store: StoreDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ServerDefaults {
    address: SocketAddr,
    worker_id: String,
    read_only: bool,
    max_file_count: u32,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct ClientDefaults {
    pool_size: usize,
    timeout: u64,
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
struct OldDbDefaults {
    path: PathBuf,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct MediaDefaults {
    max_file_size: usize,
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
    video_codec: VideoCodec,
    quality: VideoQualityDefaults,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct VideoQualityDefaults {
    crf_max: u8,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
enum RepoDefaults {
    Sled(SledDefaults),
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct SledDefaults {
    path: PathBuf,
    cache_capacity: u64,
    export_path: PathBuf,
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
        ServerDefaults {
            address: "0.0.0.0:8080".parse().expect("Valid address string"),
            worker_id: String::from("pict-rs-1"),
            read_only: false,
            max_file_count: 1,
        }
    }
}

impl Default for ClientDefaults {
    fn default() -> Self {
        ClientDefaults {
            pool_size: 100,
            timeout: 30,
        }
    }
}

impl Default for LoggingDefaults {
    fn default() -> Self {
        LoggingDefaults {
            format: LogFormat::Normal,
            targets: "warn,tracing_actix_web=info,actix_web=info,actix_server=info"
                .parse()
                .expect("Valid targets string"),
        }
    }
}

impl Default for ConsoleDefaults {
    fn default() -> Self {
        ConsoleDefaults {
            buffer_capacity: 1024 * 100,
        }
    }
}

impl Default for OpenTelemetryDefaults {
    fn default() -> Self {
        OpenTelemetryDefaults {
            service_name: String::from("pict-rs"),
            targets: "info".parse().expect("Valid targets string"),
        }
    }
}

impl Default for OldDbDefaults {
    fn default() -> Self {
        OldDbDefaults {
            path: PathBuf::from(String::from("/mnt")),
        }
    }
}

impl Default for MediaDefaults {
    fn default() -> Self {
        MediaDefaults {
            max_file_size: 40,
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
        RetentionDefaults {
            variants: "7d",
            proxy: "7d",
        }
    }
}

impl Default for ImageDefaults {
    fn default() -> Self {
        ImageDefaults {
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
        AnimationDefaults {
            max_height: 256,
            max_width: 256,
            max_area: 65_536,
            max_frame_count: 100,
            max_file_size: 40,
            quality: AnimationQualityDefaults {},
        }
    }
}

impl Default for VideoDefaults {
    fn default() -> Self {
        VideoDefaults {
            enable: true,
            allow_audio: false,
            max_height: 3_840,
            max_width: 3_840,
            max_area: 8_294_400,
            max_frame_count: 900,
            max_file_size: 40,
            video_codec: VideoCodec::Vp9,
            quality: VideoQualityDefaults::default(),
        }
    }
}

impl Default for VideoQualityDefaults {
    fn default() -> Self {
        VideoQualityDefaults { crf_max: 32 }
    }
}

impl Default for RepoDefaults {
    fn default() -> Self {
        Self::Sled(SledDefaults::default())
    }
}

impl Default for SledDefaults {
    fn default() -> Self {
        SledDefaults {
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
        crate::config::primitives::Store::Filesystem(value.into())
    }
}
