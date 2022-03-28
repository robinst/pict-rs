use crate::{
    config::primitives::{LogFormat, Targets},
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
    enable_silent_video: bool,
    filters: Vec<String>,
    skip_validate_imports: bool,
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
enum StoreDefaults {
    Filesystem(FilesystemDefaults),
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct FilesystemDefaults {
    path: PathBuf,
}

impl Default for ServerDefaults {
    fn default() -> Self {
        ServerDefaults {
            address: "0.0.0.0:8080".parse().expect("Valid address string"),
        }
    }
}

impl Default for LoggingDefaults {
    fn default() -> Self {
        LoggingDefaults {
            format: LogFormat::Normal,
            targets: "info".parse().expect("Valid targets string"),
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
            enable_silent_video: true,
            filters: vec![
                "identity".into(),
                "thumbnail".into(),
                "resize".into(),
                "crop".into(),
                "blur".into(),
            ],
            skip_validate_imports: false,
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
