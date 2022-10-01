use crate::{
    config::primitives::{AudioCodec, ImageFormat, LogFormat, Store, Targets, VideoCodec},
    serde_str::Serde,
};
use once_cell::sync::OnceCell;
use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ConfigFile {
    pub(crate) server: Server,

    pub(crate) tracing: Tracing,

    pub(crate) old_db: OldDb,

    pub(crate) media: Media,

    pub(crate) repo: Repo,

    pub(crate) store: Store,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) preprocess_steps: Option<String>,

    pub(crate) max_width: usize,

    pub(crate) max_height: usize,

    pub(crate) max_area: usize,

    pub(crate) max_file_size: usize,

    pub(crate) max_frame_count: usize,

    pub(crate) enable_silent_video: bool,

    pub(crate) enable_full_video: bool,

    pub(crate) video_codec: VideoCodec,

    pub(crate) audio_codec: Option<AudioCodec>,

    pub(crate) filters: BTreeSet<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<ImageFormat>,

    pub(crate) skip_validate_imports: bool,

    pub(crate) cache_duration: i64,
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
}
