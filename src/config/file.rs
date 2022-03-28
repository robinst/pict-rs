use crate::{
    config::primitives::{ImageFormat, LogFormat, Targets},
    serde_str::Serde,
};
use std::{net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ConfigFile {
    pub(crate) server: Server,

    pub(crate) tracing: Tracing,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) old_db: Option<OldDb>,

    pub(crate) media: Media,

    pub(crate) repo: Repo,

    pub(crate) store: Store,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type")]
pub(crate) enum Repo {
    Sled(Sled),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type")]
pub(crate) enum Store {
    Filesystem(Filesystem),

    ObjectStorage(ObjectStorage),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Server {
    pub(crate) address: SocketAddr,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_key: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Tracing {
    logging: Logging,

    #[serde(skip_serializing_if = "Option::is_none")]
    console: Option<Console>,

    #[serde(skip_serializing_if = "Option::is_none")]
    opentelemetry: Option<OpenTelemetry>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Logging {
    pub(crate) format: LogFormat,

    pub(crate) targets: Serde<Targets>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct OpenTelemetry {
    pub(crate) url: Url,

    pub(crate) service_name: String,

    pub(crate) targets: Serde<Targets>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Console {
    pub(crate) address: SocketAddr,
    pub(crate) buffer_capacity: usize,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct OldDb {
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Media {
    pub(crate) max_width: usize,

    pub(crate) max_height: usize,

    pub(crate) max_area: usize,

    pub(crate) max_file_size: usize,

    pub(crate) enable_silent_video: bool,

    pub(crate) filters: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<ImageFormat>,

    pub(crate) skip_validate_imports: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Filesystem {
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ObjectStorage {
    pub(crate) bucket_name: String,

    pub(crate) region: Serde<s3::Region>,

    pub(crate) access_key: String,

    pub(crate) secret_key: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) security_token: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_token: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Sled {
    pub(crate) path: PathBuf,

    pub(crate) cache_capacity: u64,
}
