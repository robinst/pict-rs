use crate::{
    config::primitives::{Filesystem, LogFormat, Targets},
    formats::{AnimationFormat, AudioCodec, ImageFormat, VideoCodec},
    serde_str::Serde,
};
use once_cell::sync::OnceCell;
use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ConfigFile {
    pub(crate) server: Server,

    pub(crate) client: Client,

    pub(crate) tracing: Tracing,

    pub(crate) old_db: OldDb,

    pub(crate) media: Media,

    pub(crate) repo: Repo,

    pub(crate) store: Store,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
// allow large enum variant - this is an instantiated-once config
#[allow(clippy::large_enum_variant)]
pub(crate) enum Store {
    Filesystem(Filesystem),
    ObjectStorage(ObjectStorage),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ObjectStorage {
    /// The base endpoint for the object storage
    ///
    /// Examples:
    /// - `http://localhost:9000`
    /// - `https://s3.dualstack.eu-west-1.amazonaws.com`
    pub(crate) endpoint: Url,

    /// Determines whether to use path style or virtualhost style for accessing objects
    ///
    /// When this is true, objects will be fetched from {endpoint}/{bucket_name}/{object}
    /// When false, objects will be fetched from {bucket_name}.{endpoint}/{object}
    pub(crate) use_path_style: bool,

    /// The bucket in which to store media
    pub(crate) bucket_name: String,

    /// The region the bucket is located in
    pub(crate) region: String,

    /// The Access Key for the user accessing the bucket
    pub(crate) access_key: String,

    /// The secret key for the user accessing the bucket
    pub(crate) secret_key: String,

    /// The session token for accessing the bucket
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_token: Option<String>,

    /// How long signatures for object storage requests are valid (in seconds)
    ///
    /// This defaults to 15 seconds
    pub(crate) signature_duration: u64,

    /// How long a client can wait on an object storage request before giving up (in seconds)
    ///
    /// This defaults to 30 seconds
    pub(crate) client_timeout: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) public_endpoint: Option<Url>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub(crate) enum Repo {
    Sled(Sled),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Server {
    pub(crate) address: SocketAddr,

    pub(crate) worker_id: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_key: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Client {
    pub(crate) pool_size: usize,

    pub(crate) timeout: u64,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Tracing {
    pub(crate) logging: Logging,

    pub(crate) console: Console,

    pub(crate) opentelemetry: OpenTelemetry,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Logging {
    pub(crate) format: LogFormat,

    pub(crate) targets: Serde<Targets>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct OpenTelemetry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<Url>,

    pub(crate) service_name: String,

    pub(crate) targets: Serde<Targets>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Console {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) address: Option<SocketAddr>,

    pub(crate) buffer_capacity: usize,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct OldDb {
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Media {
    pub(crate) max_file_size: usize,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preprocess_steps: Option<String>,

    pub(crate) filters: BTreeSet<String>,

    pub(crate) image: Image,

    pub(crate) animation: Animation,

    pub(crate) video: Video,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Image {
    pub(crate) max_width: u16,

    pub(crate) max_height: u16,

    pub(crate) max_area: u32,

    pub(crate) max_file_size: usize,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<ImageFormat>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Animation {
    pub(crate) max_width: u16,

    pub(crate) max_height: u16,

    pub(crate) max_area: u32,

    pub(crate) max_file_size: usize,

    pub(crate) max_frame_count: u32,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<AnimationFormat>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Video {
    pub(crate) enable: bool,

    pub(crate) allow_audio: bool,

    pub(crate) max_width: u16,

    pub(crate) max_height: u16,

    pub(crate) max_area: u32,

    pub(crate) max_file_size: usize,

    pub(crate) max_frame_count: u32,

    pub(crate) video_codec: VideoCodec,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) audio_codec: Option<AudioCodec>,
}

impl Media {
    pub(crate) fn preprocess_steps(&self) -> Option<&[(String, String)]> {
        static PREPROCESS_STEPS: OnceCell<Vec<(String, String)>> = OnceCell::new();

        if let Some(steps) = &self.preprocess_steps {
            let steps = PREPROCESS_STEPS
                .get_or_try_init(|| {
                    serde_urlencoded::from_str(steps) as Result<Vec<(String, String)>, _>
                })
                .expect("Invalid preprocess_steps configuration")
                .as_slice();

            Some(steps)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Sled {
    pub(crate) path: PathBuf,

    pub(crate) cache_capacity: u64,

    pub(crate) export_path: PathBuf,
}
