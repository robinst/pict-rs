use crate::{
    config::primitives::{Filesystem, LogFormat, RetentionValue, Targets},
    formats::{AnimationFormat, AudioCodec, ImageFormat, VideoCodec},
    serde_str::Serde,
};
use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ConfigFile {
    pub(crate) server: Server,

    pub(crate) client: Client,

    pub(crate) upgrade: Upgrade,

    pub(crate) tracing: Tracing,

    #[serde(default)]
    pub(crate) metrics: Metrics,

    #[serde(default)]
    old_repo: OldRepo,

    pub(crate) media: Media,

    pub(crate) repo: Repo,

    pub(crate) store: Store,
}

impl ConfigFile {
    pub(crate) fn old_repo_path(&self) -> Option<&PathBuf> {
        self.old_repo.path.as_ref().or(match self.repo {
            Repo::Sled(ref sled) => Some(&sled.path),
            _ => None,
        })
    }
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

    /// How long a client can wait on an object storage request before giving up (in seconds)
    ///
    /// This defaults to 30 seconds
    pub(crate) client_timeout: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) public_endpoint: Option<Url>,
}

impl From<crate::config::primitives::ObjectStorage> for ObjectStorage {
    fn from(value: crate::config::primitives::ObjectStorage) -> Self {
        let defaults = crate::config::defaults::ObjectStorageDefaults::default();

        Self {
            endpoint: value.endpoint,
            use_path_style: value.use_path_style,
            bucket_name: value.bucket_name,
            region: value.region,
            access_key: value.access_key,
            secret_key: value.secret_key,
            session_token: value.session_token,
            client_timeout: value.client_timeout.unwrap_or(defaults.client_timeout),
            public_endpoint: value.public_endpoint,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub(crate) enum Repo {
    Sled(Sled),
    Postgres(Postgres),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Server {
    pub(crate) address: SocketAddr,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_key: Option<String>,

    pub(crate) read_only: bool,

    pub(crate) danger_dummy_mode: bool,

    pub(crate) max_file_count: u32,

    pub(crate) temporary_directory: PathBuf,

    pub(crate) cleanup_temporary_directory: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) certificate: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) private_key: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Client {
    pub(crate) timeout: u64,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Upgrade {
    pub(crate) concurrency: usize,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Tracing {
    pub(crate) logging: Logging,

    pub(crate) console: Console,

    pub(crate) opentelemetry: OpenTelemetry,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Metrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prometheus_address: Option<SocketAddr>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Logging {
    pub(crate) format: LogFormat,

    pub(crate) targets: Serde<Targets>,

    pub(crate) log_spans: bool,

    pub(crate) no_ansi: bool,

    pub(crate) log_requests: bool,
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
    pub(crate) external_validation: Option<Url>,

    pub(crate) external_validation_timeout: u64,

    pub(crate) max_file_size: usize,

    pub(crate) process_timeout: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    preprocess_steps: Option<PreprocessSteps>,

    pub(crate) filters: BTreeSet<String>,

    pub(crate) retention: Retention,

    pub(crate) magick: Magick,

    pub(crate) image: Image,

    pub(crate) animation: Animation,

    pub(crate) video: Video,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Retention {
    pub(crate) variants: RetentionValue,

    pub(crate) proxy: RetentionValue,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Magick {
    pub(crate) max_width: u16,

    pub(crate) max_height: u16,

    pub(crate) max_area: u32,

    pub(crate) memory: u32,

    pub(crate) map: u32,

    pub(crate) disk: u32,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Image {
    pub(crate) max_width: u16,

    pub(crate) max_height: u16,

    pub(crate) max_area: u32,

    pub(crate) max_file_size: usize,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<ImageFormat>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quality: Option<ImageQuality>,
}

impl Image {
    pub(crate) fn quality_for(&self, format: ImageFormat) -> Option<u8> {
        self.quality
            .as_ref()
            .and_then(|quality| match format {
                ImageFormat::Avif => quality.avif,
                ImageFormat::Jpeg => quality.jpeg,
                ImageFormat::Jxl => quality.jxl,
                ImageFormat::Png => quality.png,
                ImageFormat::Webp => quality.webp,
            })
            .map(|quality| quality.min(100))
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ImageQuality {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) avif: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) png: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) jpeg: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) jxl: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) webp: Option<u8>,
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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quality: Option<AnimationQuality>,
}

impl Animation {
    pub(crate) fn quality_for(&self, format: AnimationFormat) -> Option<u8> {
        self.quality
            .as_ref()
            .and_then(|quality| match format {
                AnimationFormat::Apng => quality.apng,
                AnimationFormat::Avif => quality.avif,
                AnimationFormat::Gif => None,
                AnimationFormat::Webp => quality.webp,
            })
            .map(|quality| quality.min(100))
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct AnimationQuality {
    #[serde(skip_serializing_if = "Option::is_none")]
    apng: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    avif: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    webp: Option<u8>,
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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) video_codec: Option<VideoCodec>,

    pub(crate) quality: VideoQuality,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) audio_codec: Option<AudioCodec>,
}

impl Video {
    pub(crate) fn crf_for(&self, width: u16, height: u16) -> u8 {
        let smaller_dimension = width.min(height);

        let dimension_cutoffs = [240, 360, 480, 720, 1080, 1440, 2160];
        let crfs = [
            self.quality.crf_240,
            self.quality.crf_360,
            self.quality.crf_480,
            self.quality.crf_720,
            self.quality.crf_1080,
            self.quality.crf_1440,
            self.quality.crf_2160,
        ];

        let index = dimension_cutoffs
            .into_iter()
            .enumerate()
            .find_map(|(index, dim)| {
                if smaller_dimension <= dim {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(crfs.len());

        crfs.into_iter()
            .skip(index)
            .find_map(|opt| opt)
            .unwrap_or(self.quality.crf_max)
            .min(63)
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct VideoQuality {
    #[serde(skip_serializing_if = "Option::is_none")]
    crf_240: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    crf_360: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    crf_480: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    crf_720: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    crf_1080: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    crf_1440: Option<u8>,

    #[serde(skip_serializing_if = "Option::is_none")]
    crf_2160: Option<u8>,

    crf_max: u8,
}

#[derive(Clone, Debug)]
struct PreprocessSteps {
    inner: Vec<(String, String)>,
}

impl<'de> serde::Deserialize<'de> for PreprocessSteps {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let s = String::deserialize(deserializer)?;

        let inner: Vec<(String, String)> =
            serde_urlencoded::from_str(&s).map_err(D::Error::custom)?;

        Ok(PreprocessSteps { inner })
    }
}

impl serde::Serialize for PreprocessSteps {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error;

        let s = serde_urlencoded::to_string(&self.inner).map_err(S::Error::custom)?;

        s.serialize(serializer)
    }
}

impl Media {
    pub(crate) fn preprocess_steps(&self) -> Option<&[(String, String)]> {
        self.preprocess_steps
            .as_ref()
            .map(|steps| steps.inner.as_slice())
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct OldRepo {
    pub(crate) path: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Sled {
    pub(crate) path: PathBuf,

    pub(crate) cache_capacity: u64,

    pub(crate) export_path: PathBuf,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Postgres {
    pub(crate) url: Url,
    pub(crate) use_tls: bool,
    pub(crate) certificate_file: Option<PathBuf>,
}
