use crate::{
    config::primitives::{LogFormat, Targets, VideoCodec},
    serde_str::Serde,
};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Defaults {
    server: ServerDefaults,
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
    client_pool_size: usize,
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
    max_width: usize,
    max_height: usize,
    max_area: usize,
    max_file_size: usize,
    max_frame_count: usize,
    gif: GifDefaults,
    enable_silent_video: bool,
    enable_full_video: bool,
    video_codec: VideoCodec,
    filters: Vec<String>,
    skip_validate_imports: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct GifDefaults {
    max_height: usize,
    max_width: usize,
    max_area: usize,
    max_frame_count: usize,
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
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub(super) enum StoreDefaults {
    Filesystem(FilesystemDefaults),
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct FilesystemDefaults {
    path: PathBuf,
}

impl Default for ServerDefaults {
    fn default() -> Self {
        ServerDefaults {
            address: "0.0.0.0:8080".parse().expect("Valid address string"),
            worker_id: String::from("pict-rs-1"),
            client_pool_size: 100,
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
            max_width: 10_000,
            max_height: 10_000,
            max_area: 40_000_000,
            max_file_size: 40,
            max_frame_count: 900,
            gif: Default::default(),
            enable_silent_video: true,
            enable_full_video: false,
            video_codec: VideoCodec::Vp9,
            filters: vec![
                "blur".into(),
                "crop".into(),
                "identity".into(),
                "resize".into(),
                "thumbnail".into(),
            ],
            skip_validate_imports: false,
        }
    }
}

impl Default for GifDefaults {
    fn default() -> Self {
        GifDefaults {
            max_height: 128,
            max_width: 128,
            max_area: 16384,
            max_frame_count: 100,
        }
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
        }
    }
}

impl Default for StoreDefaults {
    fn default() -> Self {
        Self::Filesystem(FilesystemDefaults::default())
    }
}

impl Default for FilesystemDefaults {
    fn default() -> Self {
        Self {
            path: PathBuf::from(String::from("/mnt/files")),
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
