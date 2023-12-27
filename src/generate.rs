mod ffmpeg;
mod magick;

use crate::{
    concurrent_processor::ProcessMap,
    details::Details,
    error::{Error, UploadError},
    formats::{ImageFormat, InputProcessableFormat, InternalVideoFormat, ProcessableFormat},
    future::{WithMetrics, WithTimeout},
    repo::{ArcRepo, Hash, VariantAlreadyExists},
    store::Store,
    tmp_file::TmpDir,
};
use actix_web::web::Bytes;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use tracing::Instrument;

struct MetricsGuard {
    start: Instant,
    armed: bool,
}

impl MetricsGuard {
    fn guard() -> Self {
        metrics::counter!("pict-rs.generate.start").increment(1);
        Self {
            start: Instant::now(),
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for MetricsGuard {
    fn drop(&mut self) {
        metrics::histogram!("pict-rs.generate.duration", "completed" => (!self.armed).to_string())
            .record(self.start.elapsed().as_secs_f64());
        metrics::counter!("pict-rs.generate.end", "completed" => (!self.armed).to_string())
            .increment(1);
    }
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(tmp_dir, repo, store, hash, process_map, config))]
pub(crate) async fn generate<S: Store + 'static>(
    tmp_dir: &TmpDir,
    repo: &ArcRepo,
    store: &S,
    process_map: &ProcessMap,
    format: InputProcessableFormat,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    original_details: &Details,
    config: &crate::config::Configuration,
    hash: Hash,
) -> Result<(Details, Bytes), Error> {
    if config.server.danger_dummy_mode {
        let identifier = repo
            .identifier(hash)
            .await?
            .ok_or(UploadError::MissingIdentifier)?;

        let bytes = store.to_bytes(&identifier, None, None).await?.into_bytes();

        Ok((original_details.clone(), bytes))
    } else {
        let process_fut = process(
            tmp_dir,
            repo,
            store,
            format,
            thumbnail_path.clone(),
            thumbnail_args,
            original_details,
            config,
            hash.clone(),
        );

        let (details, bytes) = process_map
            .process(hash, thumbnail_path, process_fut)
            .with_timeout(Duration::from_secs(config.media.process_timeout * 4))
            .with_metrics("pict-rs.generate.process")
            .await
            .map_err(|_| UploadError::ProcessTimeout)??;

        Ok((details, bytes))
    }
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(tmp_dir, repo, store, hash, config))]
async fn process<S: Store + 'static>(
    tmp_dir: &TmpDir,
    repo: &ArcRepo,
    store: &S,
    output_format: InputProcessableFormat,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    original_details: &Details,
    config: &crate::config::Configuration,
    hash: Hash,
) -> Result<(Details, Bytes), Error> {
    let guard = MetricsGuard::guard();
    let permit = crate::process_semaphore().acquire().await?;

    let identifier = input_identifier(
        tmp_dir,
        repo,
        store,
        output_format,
        hash.clone(),
        original_details,
        &config.media,
    )
    .await?;

    let input_details =
        crate::ensure_details_identifier(tmp_dir, repo, store, config, &identifier).await?;

    let input_format = input_details
        .internal_format()
        .processable_format()
        .expect("Already verified format is processable");

    let format = input_format.process_to(output_format);

    let quality = match format {
        ProcessableFormat::Image(format) => config.media.image.quality_for(format),
        ProcessableFormat::Animation(format) => config.media.animation.quality_for(format),
    };

    let stream = store.to_stream(&identifier, None, None).await?;

    let vec = crate::magick::process_image_stream_read(
        tmp_dir,
        stream,
        thumbnail_args,
        input_format,
        format,
        quality,
        config.media.process_timeout,
    )
    .await?
    .into_vec()
    .instrument(tracing::info_span!("Reading processed image to vec"))
    .await?;

    let bytes = Bytes::from(vec);

    drop(permit);

    let details = Details::from_bytes(tmp_dir, config.media.process_timeout, bytes.clone()).await?;

    let identifier = store
        .save_bytes(bytes.clone(), details.media_type())
        .await?;

    if let Err(VariantAlreadyExists) = repo
        .relate_variant_identifier(
            hash,
            thumbnail_path.to_string_lossy().to_string(),
            &identifier,
        )
        .await?
    {
        store.remove(&identifier).await?;
    }

    repo.relate_details(&identifier, &details).await?;

    guard.disarm();

    Ok((details, bytes)) as Result<(Details, Bytes), Error>
}

#[tracing::instrument(skip_all)]
async fn input_identifier<S>(
    tmp_dir: &TmpDir,
    repo: &ArcRepo,
    store: &S,
    output_format: InputProcessableFormat,
    hash: Hash,
    original_details: &Details,
    media: &crate::config::Media,
) -> Result<Arc<str>, Error>
where
    S: Store + 'static,
{
    let should_thumbnail =
        if let Some(input_format) = original_details.internal_format().processable_format() {
            let output_format = input_format.process_to(output_format);

            input_format.should_thumbnail(output_format)
        } else {
            // video case
            true
        };

    if should_thumbnail {
        if let Some(identifier) = repo.motion_identifier(hash.clone()).await? {
            return Ok(identifier);
        };

        let identifier = repo
            .identifier(hash.clone())
            .await?
            .ok_or(UploadError::MissingIdentifier)?;

        let (reader, media_type) = if let Some(processable_format) =
            original_details.internal_format().processable_format()
        {
            let thumbnail_format = media.image.format.unwrap_or(ImageFormat::Webp);

            let stream = store.to_stream(&identifier, None, None).await?;

            let reader = magick::thumbnail(
                tmp_dir,
                stream,
                processable_format,
                ProcessableFormat::Image(thumbnail_format),
                media.image.quality_for(thumbnail_format),
                media.process_timeout,
            )
            .await?;

            (reader, thumbnail_format.media_type())
        } else {
            let thumbnail_format = match media.image.format {
                Some(ImageFormat::Webp | ImageFormat::Avif | ImageFormat::Jxl) => {
                    ffmpeg::ThumbnailFormat::Webp
                }
                Some(ImageFormat::Png) => ffmpeg::ThumbnailFormat::Png,
                Some(ImageFormat::Jpeg) | None => ffmpeg::ThumbnailFormat::Jpeg,
            };

            let reader = ffmpeg::thumbnail(
                tmp_dir,
                store.clone(),
                identifier,
                original_details
                    .video_format()
                    .unwrap_or(InternalVideoFormat::Mp4),
                thumbnail_format,
                media.process_timeout,
            )
            .await?;

            (reader, thumbnail_format.media_type())
        };

        let motion_identifier = reader
            .with_stdout(|stdout| async { store.save_async_read(stdout, media_type).await })
            .await??;

        repo.relate_motion_identifier(hash, &motion_identifier)
            .await?;

        return Ok(motion_identifier);
    }

    repo.identifier(hash)
        .await?
        .ok_or(UploadError::MissingIdentifier)
        .map_err(From::from)
}
