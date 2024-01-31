use crate::{
    config::primitives::{LogFormat, Targets},
    formats::{AnimationFormat, AudioCodec, ImageFormat, VideoCodec},
    serde_str::Serde,
};
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};
use url::Url;

use super::primitives::RetentionValue;

impl Args {
    pub(super) fn into_output(self) -> Output {
        let Args {
            config_file,
            old_repo_path,
            old_repo_cache_capacity,
            log_format,
            log_targets,
            log_spans,
            console_address,
            console_buffer_capacity,
            opentelemetry_url,
            opentelemetry_service_name,
            opentelemetry_targets,
            save_to,
            command,
        } = self;

        let old_repo = OldSled {
            path: old_repo_path,
            cache_capacity: old_repo_cache_capacity,
        }
        .set();

        let tracing = Tracing {
            logging: Logging {
                format: log_format,
                targets: log_targets.map(Serde::new),
                log_spans,
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
                temporary_directory,
                certificate,
                private_key,
                client_timeout,
                upgrade_concurrency,
                metrics_prometheus_address,
                media_preprocess_steps,
                media_external_validation,
                media_external_validation_timeout,
                media_max_file_size,
                media_process_timeout,
                media_retention_variants,
                media_retention_proxy,
                media_image_max_width,
                media_image_max_height,
                media_image_max_area,
                media_image_max_file_size,
                media_image_format,
                media_image_quality_avif,
                media_image_quality_jpeg,
                media_image_quality_jxl,
                media_image_quality_png,
                media_image_quality_webp,
                media_animation_max_width,
                media_animation_max_height,
                media_animation_max_area,
                media_animation_max_file_size,
                media_animation_max_frame_count,
                media_animation_format,
                media_animation_quality_apng,
                media_animation_quality_avif,
                media_animation_quality_webp,
                media_video_disable,
                media_video_allow_audio,
                media_video_max_width,
                media_video_max_height,
                media_video_max_area,
                media_video_max_file_size,
                media_video_max_frame_count,
                media_video_codec,
                media_video_audio_codec,
                media_video_quality_max,
                media_video_quality_240,
                media_video_quality_360,
                media_video_quality_480,
                media_video_quality_720,
                media_video_quality_1080,
                media_video_quality_1440,
                media_video_quality_2160,
                media_filters,
                read_only,
                danger_dummy_mode,
                max_file_count,
                store,
            }) => {
                let server = Server {
                    address,
                    api_key,
                    read_only,
                    danger_dummy_mode,
                    max_file_count,
                    temporary_directory,
                    certificate,
                    private_key,
                };

                let client = Client {
                    timeout: client_timeout,
                };

                let upgrade = Upgrade {
                    concurrency: upgrade_concurrency,
                };

                let metrics = Metrics {
                    prometheus_address: metrics_prometheus_address,
                };

                let retention = Retention {
                    variants: media_retention_variants,
                    proxy: media_retention_proxy,
                };

                let image_quality = ImageQuality {
                    avif: media_image_quality_avif,
                    jpeg: media_image_quality_jpeg,
                    jxl: media_image_quality_jxl,
                    png: media_image_quality_png,
                    webp: media_image_quality_webp,
                };

                let image = Image {
                    max_file_size: media_image_max_file_size,
                    max_width: media_image_max_width,
                    max_height: media_image_max_height,
                    max_area: media_image_max_area,
                    format: media_image_format,
                    quality: image_quality.set(),
                };

                let animation_quality = AnimationQuality {
                    apng: media_animation_quality_apng,
                    avif: media_animation_quality_avif,
                    webp: media_animation_quality_webp,
                };

                let animation = Animation {
                    max_file_size: media_animation_max_file_size,
                    max_width: media_animation_max_width,
                    max_height: media_animation_max_height,
                    max_area: media_animation_max_area,
                    max_frame_count: media_animation_max_frame_count,
                    format: media_animation_format,
                    quality: animation_quality.set(),
                };

                let video_quality = VideoQuality {
                    crf_240: media_video_quality_240,
                    crf_360: media_video_quality_360,
                    crf_480: media_video_quality_480,
                    crf_720: media_video_quality_720,
                    crf_1080: media_video_quality_1080,
                    crf_1440: media_video_quality_1440,
                    crf_2160: media_video_quality_2160,
                    crf_max: media_video_quality_max,
                };

                let video = Video {
                    enable: !media_video_disable,
                    allow_audio: media_video_allow_audio,
                    max_file_size: media_video_max_file_size,
                    max_width: media_video_max_width,
                    max_height: media_video_max_height,
                    max_area: media_video_max_area,
                    max_frame_count: media_video_max_frame_count,
                    video_codec: media_video_codec,
                    audio_codec: media_video_audio_codec,
                    quality: video_quality.set(),
                };

                let media = Media {
                    max_file_size: media_max_file_size,
                    process_timeout: media_process_timeout,
                    preprocess_steps: media_preprocess_steps,
                    external_validation: media_external_validation,
                    external_validation_timeout: media_external_validation_timeout,
                    filters: media_filters,
                    retention: retention.set(),
                    image: image.set(),
                    animation: animation.set(),
                    video: video.set(),
                };
                let operation = Operation::Run;

                match store {
                    Some(RunStore::Filesystem(RunFilesystem { system, repo })) => {
                        let store = Some(Store::Filesystem(system));
                        Output {
                            config_format: ConfigFormat {
                                server,
                                client,
                                upgrade,
                                old_repo,
                                tracing,
                                metrics,
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
                                client,
                                upgrade,
                                old_repo,
                                tracing,
                                metrics,
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
                            client,
                            upgrade,
                            old_repo,
                            tracing,
                            metrics,
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
                concurrency,
                store,
            }) => {
                let server = Server::default();
                let client = Client::default();
                let upgrade = Upgrade::default();
                let media = Media::default();
                let metrics = Metrics::default();

                match store {
                    MigrateStoreFrom::Filesystem(MigrateFilesystem { from, to }) => match to {
                        MigrateStoreTo::Filesystem(MigrateFilesystemInner { to, repo }) => Output {
                            config_format: ConfigFormat {
                                server,
                                client,
                                upgrade,
                                old_repo,
                                tracing,
                                metrics,
                                media,
                                store: None,
                                repo,
                            },
                            operation: Operation::MigrateStore {
                                skip_missing_files,
                                concurrency,
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
                                    client,
                                    upgrade,
                                    old_repo,
                                    tracing,
                                    metrics,
                                    media,
                                    store: None,
                                    repo,
                                },
                                operation: Operation::MigrateStore {
                                    skip_missing_files,
                                    concurrency,
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
                                        client,
                                        upgrade,
                                        old_repo,
                                        tracing,
                                        metrics,
                                        media,
                                        store: None,
                                        repo,
                                    },
                                    operation: Operation::MigrateStore {
                                        skip_missing_files,
                                        concurrency,
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
                                    client,
                                    upgrade,
                                    old_repo,
                                    tracing,
                                    metrics,
                                    media,
                                    store: None,
                                    repo,
                                },
                                operation: Operation::MigrateStore {
                                    skip_missing_files,
                                    concurrency,
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
            Command::MigrateRepo(MigrateRepo { repo }) => {
                let server = Server::default();
                let client = Client::default();
                let upgrade = Upgrade::default();
                let media = Media::default();
                let metrics = Metrics::default();

                match repo {
                    MigrateRepoFrom::Sled(MigrateSledRepo { from, to }) => match to {
                        MigrateRepoTo::Sled(MigrateSledInner { to }) => Output {
                            config_format: ConfigFormat {
                                server,
                                client,
                                upgrade,
                                old_repo,
                                tracing,
                                metrics,
                                media,
                                repo: None,
                                store: None,
                            },
                            operation: Operation::MigrateRepo {
                                from: from.into(),
                                to: to.into(),
                            },
                            save_to,
                            config_file,
                        },
                        MigrateRepoTo::Postgres(MigratePostgresInner { to }) => Output {
                            config_format: ConfigFormat {
                                server,
                                client,
                                upgrade,
                                old_repo,
                                tracing,
                                metrics,
                                media,
                                repo: None,
                                store: None,
                            },
                            operation: Operation::MigrateRepo {
                                from: from.into(),
                                to: to.into(),
                            },
                            save_to,
                            config_file,
                        },
                    },
                    MigrateRepoFrom::Postgres(MigratePostgresRepo { from, to }) => match to {
                        MigrateRepoTo::Sled(MigrateSledInner { to }) => Output {
                            config_format: ConfigFormat {
                                server,
                                client,
                                upgrade,
                                old_repo,
                                tracing,
                                metrics,
                                media,
                                repo: None,
                                store: None,
                            },
                            operation: Operation::MigrateRepo {
                                from: from.into(),
                                to: to.into(),
                            },
                            save_to,
                            config_file,
                        },
                        MigrateRepoTo::Postgres(MigratePostgresInner { to }) => Output {
                            config_format: ConfigFormat {
                                server,
                                client,
                                upgrade,
                                old_repo,
                                tracing,
                                metrics,
                                media,
                                repo: None,
                                store: None,
                            },
                            operation: Operation::MigrateRepo {
                                from: from.into(),
                                to: to.into(),
                            },
                            save_to,
                            config_file,
                        },
                    },
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
#[derive(Clone, Debug)]
pub(crate) enum Operation {
    Run,
    MigrateStore {
        skip_missing_files: bool,
        concurrency: usize,
        from: crate::config::primitives::Store,
        to: crate::config::primitives::Store,
    },
    MigrateRepo {
        from: crate::config::file::Repo,
        to: crate::config::file::Repo,
    },
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct ConfigFormat {
    server: Server,
    client: Client,
    upgrade: Upgrade,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_repo: Option<OldSled>,
    tracing: Tracing,
    metrics: Metrics,
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
    api_key: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    read_only: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    danger_dummy_mode: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temporary_directory: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    certificate: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    private_key: Option<PathBuf>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Client {
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Upgrade {
    #[serde(skip_serializing_if = "Option::is_none")]
    concurrency: Option<usize>,
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    log_spans: bool,
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
struct Metrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    prometheus_address: Option<SocketAddr>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Media {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preprocess_steps: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_validation: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_validation_timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retention: Option<Retention>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Image>,
    #[serde(skip_serializing_if = "Option::is_none")]
    animation: Option<Animation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video: Option<Video>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Retention {
    #[serde(skip_serializing_if = "Option::is_none")]
    variants: Option<RetentionValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy: Option<RetentionValue>,
}

impl Retention {
    fn set(self) -> Option<Self> {
        let any_set = self.variants.is_some() || self.proxy.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Image {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_width: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_height: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_area: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<ImageFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<ImageQuality>,
}

impl Image {
    fn set(self) -> Option<Self> {
        let any_set = self.max_width.is_some()
            || self.max_height.is_some()
            || self.max_area.is_some()
            || self.max_file_size.is_some()
            || self.format.is_some()
            || self.quality.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
struct ImageQuality {
    #[serde(skip_serializing_if = "Option::is_none")]
    avif: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jpeg: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jxl: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    png: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    webp: Option<u8>,
}

impl ImageQuality {
    fn set(self) -> Option<Self> {
        let any_set = self.avif.is_some()
            || self.jpeg.is_some()
            || self.jxl.is_some()
            || self.png.is_some()
            || self.webp.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Animation {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_width: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_height: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_area: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_frame_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<AnimationFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<AnimationQuality>,
}

impl Animation {
    fn set(self) -> Option<Self> {
        let any_set = self.max_width.is_some()
            || self.max_height.is_some()
            || self.max_area.is_some()
            || self.max_frame_count.is_some()
            || self.max_file_size.is_some()
            || self.format.is_some()
            || self.quality.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct AnimationQuality {
    #[serde(skip_serializing_if = "Option::is_none")]
    apng: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avif: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    webp: Option<u8>,
}

impl AnimationQuality {
    fn set(self) -> Option<Self> {
        let any_set = self.apng.is_some() || self.avif.is_some() || self.webp.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
}

fn is_set(input: &bool) -> bool {
    *input
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct Video {
    #[serde(skip_serializing_if = "is_set")]
    enable: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    allow_audio: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_width: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_height: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_area: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_frame_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_file_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video_codec: Option<VideoCodec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_codec: Option<AudioCodec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<VideoQuality>,
}

impl Video {
    fn set(self) -> Option<Self> {
        let any_set = self.enable
            || self.allow_audio
            || self.max_width.is_some()
            || self.max_height.is_some()
            || self.max_area.is_some()
            || self.max_frame_count.is_some()
            || self.max_file_size.is_some()
            || self.video_codec.is_some()
            || self.audio_codec.is_some()
            || self.quality.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct VideoQuality {
    crf_240: Option<u8>,
    crf_360: Option<u8>,
    crf_480: Option<u8>,
    crf_720: Option<u8>,
    crf_1080: Option<u8>,
    crf_1440: Option<u8>,
    crf_2160: Option<u8>,
    crf_max: Option<u8>,
}

impl VideoQuality {
    fn set(self) -> Option<Self> {
        let any_set = self.crf_240.is_some()
            || self.crf_360.is_some()
            || self.crf_480.is_some()
            || self.crf_720.is_some()
            || self.crf_1080.is_some()
            || self.crf_1440.is_some()
            || self.crf_1440.is_some()
            || self.crf_2160.is_some()
            || self.crf_max.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
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
    old_repo_path: Option<PathBuf>,

    /// The cache capacity, in bytes, allowed to sled for in-memory operations
    #[arg(long)]
    old_repo_cache_capacity: Option<u64>,

    /// Format of logs printed to stdout
    #[arg(long)]
    log_format: Option<LogFormat>,
    /// Log levels to print to stdout, respects RUST_LOG formatting
    #[arg(long)]
    log_targets: Option<Targets>,
    /// Whether to log openning and closing of tracing spans to stdout
    #[arg(long)]
    log_spans: bool,

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

    /// Migrates from one provided repo to another
    MigrateRepo(MigrateRepo),
}

#[derive(Debug, Parser)]
struct Run {
    /// The address and port to bind the pict-rs web server
    #[arg(short, long)]
    address: Option<SocketAddr>,

    /// The API KEY required to access restricted routes
    #[arg(long)]
    api_key: Option<String>,

    /// The temporary directory pict-rs should use when processing media
    #[arg(long)]
    temporary_directory: Option<PathBuf>,

    /// The path to the TLS certificate. Both the certificate and the private_key must be specified
    /// to enable TLS
    #[arg(long)]
    certificate: Option<PathBuf>,

    /// The path to the private key used to negotiate TLS. Both the private_key and the certificate
    /// must be specified to enable TLS
    #[arg(long)]
    private_key: Option<PathBuf>,

    /// How long (in seconds) the internel HTTP client should wait for responses
    ///
    /// This number defaults to 30
    #[arg(long)]
    client_timeout: Option<u64>,

    /// How many hashes pict-rs should try to migrate from 0.4 to 0.5 concurrently
    ///
    /// This number defaults to 32, but can be increased for better throughput
    #[arg(long)]
    upgrade_concurrency: Option<usize>,

    /// Whether to enable the prometheus scrape endpoint
    #[arg(long)]
    metrics_prometheus_address: Option<SocketAddr>,

    /// How many files are allowed to be uploaded per-request
    ///
    /// This number defaults to 1
    #[arg(long)]
    max_file_count: Option<u32>,

    /// Optional pre-processing steps for uploaded media.
    ///
    /// All still images will be put through these steps before saving
    #[arg(long)]
    media_preprocess_steps: Option<String>,

    /// Optional endpoint to submit uploaded media to for validation
    #[arg(long)]
    media_external_validation: Option<Url>,

    /// Timeout for requests to the external validation endpoint
    #[arg(long)]
    media_external_validation_timeout: Option<u64>,

    /// Which media filters should be enabled on the `process` endpoint
    #[arg(long)]
    media_filters: Option<Vec<String>>,

    /// The maximum size, in megabytes, for all uploaded media
    #[arg(long)]
    media_max_file_size: Option<usize>,

    /// Timeout for any media processing operation
    #[arg(long)]
    media_process_timeout: Option<u64>,

    /// How long to keep image "variants" around
    ///
    /// A variant is any processed version of an original image
    #[arg(long)]
    media_retention_variants: Option<RetentionValue>,

    /// How long to keep "proxied" images around
    ///
    /// Proxied images are any images ingested using the media proxy functionality
    #[arg(long)]
    media_retention_proxy: Option<RetentionValue>,

    /// The maximum width, in pixels, for uploaded images
    #[arg(long)]
    media_image_max_width: Option<u16>,
    /// The maximum height, in pixels, for uploaded images
    #[arg(long)]
    media_image_max_height: Option<u16>,
    /// The maximum area, in pixels, for uploaded images
    #[arg(long)]
    media_image_max_area: Option<u32>,
    /// The maximum size, in megabytes, for uploaded images
    #[arg(long)]
    media_image_max_file_size: Option<usize>,
    /// Enforce a specific format for uploaded images
    #[arg(long)]
    media_image_format: Option<ImageFormat>,
    /// Enforce a specific quality for AVIF images
    ///
    /// A higher number means better quality, with a minimum value of 0 and a maximum value of 100
    #[arg(long)]
    media_image_quality_avif: Option<u8>,
    /// Enforce a specific compression level for PNG images
    ///
    /// A higher number means better compression. PNGs will look the same regardless
    #[arg(long)]
    media_image_quality_png: Option<u8>,
    /// Enforce a specific quality for JPEG images
    ///
    /// A higher number means better quality, with a minimum value of 0 and a maximum value of 100
    #[arg(long)]
    media_image_quality_jpeg: Option<u8>,
    /// Enforce a specific quality for JXL images
    ///
    /// A higher number means better quality, with a minimum value of 0 and a maximum value of 100
    #[arg(long)]
    media_image_quality_jxl: Option<u8>,
    /// Enforce a specific quality for WEBP images
    ///
    /// A higher number means better quality, with a minimum value of 0 and a maximum value of 100
    #[arg(long)]
    media_image_quality_webp: Option<u8>,

    /// The maximum width, in pixels, for uploaded animations
    #[arg(long)]
    media_animation_max_width: Option<u16>,
    /// The maximum height, in pixels, for uploaded animations
    #[arg(long)]
    media_animation_max_height: Option<u16>,
    /// The maximum area, in pixels, for uploaded animations
    #[arg(long)]
    media_animation_max_area: Option<u32>,
    /// The maximum number of frames allowed for uploaded animations
    #[arg(long)]
    media_animation_max_frame_count: Option<u32>,
    /// The maximum size, in megabytes, for uploaded animations
    #[arg(long)]
    media_animation_max_file_size: Option<usize>,
    /// Enforce a specific format for uploaded animations
    #[arg(long)]
    media_animation_format: Option<AnimationFormat>,
    /// Enforce a specific compression level for APNG animations
    ///
    /// A higher number means better compression, APNGs will look the same regardless
    #[arg(long)]
    media_animation_quality_apng: Option<u8>,
    /// Enforce a specific quality for AVIF animations
    ///
    /// A higher number means better quality, with a minimum value of 0 and a maximum value of 100
    #[arg(long)]
    media_animation_quality_avif: Option<u8>,
    /// Enforce a specific quality for WEBP animations
    ///
    /// A higher number means better quality, with a minimum value of 0 and a maximum value of 100
    #[arg(long)]
    media_animation_quality_webp: Option<u8>,

    /// Whether to disable video uploads (enabled by default)
    #[arg(long)]
    media_video_disable: bool,
    /// Whether to enable audio in video uploads
    #[arg(long)]
    media_video_allow_audio: bool,
    /// The maximum width, in pixels, for uploaded videos
    #[arg(long)]
    media_video_max_width: Option<u16>,
    /// The maximum height, in pixels, for uploaded videos
    #[arg(long)]
    media_video_max_height: Option<u16>,
    /// The maximum area, in pixels, for uploaded videos
    #[arg(long)]
    media_video_max_area: Option<u32>,
    /// The maximum number of frames allowed for uploaded videos
    #[arg(long)]
    media_video_max_frame_count: Option<u32>,
    /// The maximum size, in megabytes, for uploaded videos
    #[arg(long)]
    media_video_max_file_size: Option<usize>,
    /// Enforce a specific video codec for uploaded videos
    #[arg(long)]
    media_video_codec: Option<VideoCodec>,
    /// Enforce a specific audio codec for uploaded videos
    #[arg(long)]
    media_video_audio_codec: Option<AudioCodec>,
    /// Enforce a maximum quality level for uploaded videos
    ///
    /// This value means different things for different video codecs:
    /// - it ranges from 0 to 63 for AV1
    /// - it ranges from 4 to 63 for VP8
    /// - it ranges from 0 to 63 for VP9
    /// - it ranges from 0 to 51 for H265
    /// - it ranges from 0 to 51 for 8bit H264
    /// - it ranges from 0 to 63 for 10bit H264
    ///
    /// A lower value (closer to 0) is higher quality, while a higher value (closer to 63) is lower
    /// quality. Generally acceptable ranges are 15-38, where lower values are preferred for larger
    /// videos
    #[arg(long)]
    media_video_quality_max: Option<u8>,
    /// Enforce a video quality for video with a smaller dimension less than 240px
    #[arg(long)]
    media_video_quality_240: Option<u8>,
    /// Enforce a video quality for video with a smaller dimension less than 360px
    #[arg(long)]
    media_video_quality_360: Option<u8>,
    /// Enforce a video quality for video with a smaller dimension less than 480px
    #[arg(long)]
    media_video_quality_480: Option<u8>,
    /// Enforce a video quality for video with a smaller dimension less than 720px
    #[arg(long)]
    media_video_quality_720: Option<u8>,
    /// Enforce a video quality for video with a smaller dimension less than 1080px
    #[arg(long)]
    media_video_quality_1080: Option<u8>,
    /// Enforce a video quality for video with a smaller dimension less than 1440px
    #[arg(long)]
    media_video_quality_1440: Option<u8>,
    /// Enforce a video quality for video with a smaller dimension less than 2160px
    #[arg(long)]
    media_video_quality_2160: Option<u8>,

    /// Don't permit ingesting media
    #[arg(long)]
    read_only: bool,

    /// Allow running without ffmpeg, imagemagick, or exiftool. This will allow hosting arbitrary
    /// files and provide inaccurate metadata for uploaded media
    #[arg(long)]
    danger_dummy_mode: bool,

    #[command(subcommand)]
    store: Option<RunStore>,
}

/// Configure the provided storage
#[derive(Clone, Debug, Subcommand, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
// allow large enum variant - this is an instantiated-once config
#[allow(clippy::large_enum_variant)]
enum Store {
    /// configure filesystem storage
    Filesystem(Filesystem),

    /// configure object storage
    ObjectStorage(ObjectStorage),
}

/// Run pict-rs with the provided storage
#[derive(Debug, Subcommand)]
// allow large enum variant - this is an instantiated-once config
#[allow(clippy::large_enum_variant)]
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

    /// How many hashes pict-rs should attempt to migrate at the same time. This does not
    /// correspond to a thread count, but instead how many in-flight migrations can happen.
    /// Increasing this number may improve throughput
    #[arg(long, default_value = "32")]
    concurrency: usize,

    #[command(subcommand)]
    store: MigrateStoreFrom,
}

#[derive(Debug, Parser)]
struct MigrateRepo {
    #[command(subcommand)]
    repo: MigrateRepoFrom,
}

/// Configure the pict-rs storage migration
#[derive(Debug, Subcommand)]
// allow large enum variant - this is an instantiated-once config
#[allow(clippy::large_enum_variant)]
enum MigrateStoreFrom {
    /// Migrate from the provided filesystem storage
    Filesystem(MigrateFilesystem),

    /// Migrate from the provided object storage
    ObjectStorage(MigrateObjectStorage),
}

/// Configure the pict-rs repo migration
#[derive(Debug, Subcommand)]
enum MigrateRepoFrom {
    Sled(MigrateSledRepo),
    Postgres(MigratePostgresRepo),
}

/// Configure the destination storage for pict-rs storage migration
#[derive(Debug, Subcommand)]
// allow large enum variant - this is an instantiated-once config
#[allow(clippy::large_enum_variant)]
enum MigrateStoreTo {
    /// Migrate to the provided filesystem storage
    Filesystem(MigrateFilesystemInner),

    /// Migrate to the provided object storage
    ObjectStorage(MigrateObjectStorageInner),
}

/// Configure the destination repo for pict-rs repo migration
#[derive(Debug, Subcommand)]
enum MigrateRepoTo {
    /// Migrate to the provided sled repo
    Sled(MigrateSledInner),
    /// Migrate to the provided postgres repo
    Postgres(MigratePostgresInner),
}

/// Migrate pict-rs' storage from the provided filesystem storage
#[derive(Debug, Parser)]
struct MigrateFilesystem {
    #[command(flatten)]
    from: Filesystem,

    #[command(subcommand)]
    to: MigrateStoreTo,
}

/// Migrate pict-rs' repo from the provided sled repo
#[derive(Debug, Parser)]
struct MigrateSledRepo {
    #[command(flatten)]
    from: Sled,

    #[command(subcommand)]
    to: MigrateRepoTo,
}

/// Migrate pict-rs' repo from the provided postgres repo
#[derive(Debug, Parser)]
struct MigratePostgresRepo {
    #[command(flatten)]
    from: Postgres,

    #[command(subcommand)]
    to: MigrateRepoTo,
}

/// Migrate pict-rs' storage to the provided filesystem storage
#[derive(Debug, Parser)]
struct MigrateFilesystemInner {
    #[command(flatten)]
    to: Filesystem,

    #[command(subcommand)]
    repo: Option<Repo>,
}

/// Migrate pict-rs' repo to the provided sled repo
#[derive(Debug, Parser)]
struct MigrateSledInner {
    #[command(flatten)]
    to: Sled,
}

/// Migrate pict-rs' repo to the provided postgres repo
#[derive(Debug, Parser)]
struct MigratePostgresInner {
    #[command(flatten)]
    to: Postgres,
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
    /// Run pict-rs with the provided postgres-backed data repository
    Postgres(Postgres),
}

/// Configuration for filesystem media storage
#[derive(Clone, Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct Filesystem {
    /// The path to store uploaded media
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    bucket_name: Option<String>,

    /// The region the bucket is located in
    ///
    /// For minio deployments, this can just be 'minio'
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<String>,

    /// The Access Key for the user accessing the bucket
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    access_key: Option<String>,

    /// The secret key for the user accessing the bucket
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    secret_key: Option<String>,

    /// The session token for accessing the bucket
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    session_token: Option<String>,

    /// How long signatures for object storage requests are valid (in seconds)
    ///
    /// This defaults to 15 seconds
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    signature_duration: Option<u64>,

    /// How long a client can wait on an object storage request before giving up (in seconds)
    ///
    /// This defaults to 30 seconds
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    client_timeout: Option<u64>,
}

/// Configuration for the sled-backed data repository
#[derive(Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct Sled {
    /// The path to store the sled database
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) path: Option<PathBuf>,

    /// The cache capacity, in bytes, allowed to sled for in-memory operations
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cache_capacity: Option<u64>,

    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) export_path: Option<PathBuf>,
}

/// Configuration for the postgres-backed data repository
#[derive(Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct Postgres {
    /// The URL of the postgres database
    #[arg(short, long)]
    pub(super) url: Url,

    /// whether to connect to postgres via TLS
    #[arg(short = 't', long)]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(super) use_tls: bool,

    /// The path to the root certificate for postgres' CA
    #[arg(short, long)]
    pub(super) certificate_file: Option<PathBuf>,
}

#[derive(Debug, Parser, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct OldSled {
    /// The path to store the sled database
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,

    /// The cache capacity, in bytes, allowed to sled for in-memory operations
    #[arg(short, long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_capacity: Option<u64>,
}

impl OldSled {
    fn set(self) -> Option<Self> {
        let any_set = self.path.is_some() || self.cache_capacity.is_some();

        if any_set {
            Some(self)
        } else {
            None
        }
    }
}
