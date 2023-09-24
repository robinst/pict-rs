mod ffmpeg;
mod magick;

use crate::{
    concurrent_processor::ProcessMap,
    details::Details,
    error::{Error, UploadError},
    formats::{ImageFormat, InputProcessableFormat, InternalVideoFormat, ProcessableFormat},
    repo::{ArcRepo, Hash, VariantAlreadyExists},
    store::Store,
};
use actix_web::web::Bytes;
use std::{path::PathBuf, sync::Arc, time::Instant};
use tokio::io::AsyncReadExt;
use tracing::Instrument;

struct MetricsGuard {
    start: Instant,
    armed: bool,
}

impl MetricsGuard {
    fn guard() -> Self {
        metrics::increment_counter!("pict-rs.generate.start");
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
        metrics::histogram!("pict-rs.generate.duration", self.start.elapsed().as_secs_f64(), "completed" => (!self.armed).to_string());
        metrics::increment_counter!("pict-rs.generate.end", "completed" => (!self.armed).to_string());
    }
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(repo, store, hash, process_map, media))]
pub(crate) async fn generate<S: Store + 'static>(
    repo: &ArcRepo,
    store: &S,
    process_map: &ProcessMap,
    format: InputProcessableFormat,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    original_details: &Details,
    media: &crate::config::Media,
    hash: Hash,
) -> Result<(Details, Bytes), Error> {
    let process_fut = process(
        repo,
        store,
        format,
        thumbnail_path.clone(),
        thumbnail_args,
        original_details,
        media,
        hash.clone(),
    );

    let (details, bytes) = process_map
        .process(hash, thumbnail_path, process_fut)
        .await?;

    Ok((details, bytes))
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(repo, store, hash, media))]
async fn process<S: Store + 'static>(
    repo: &ArcRepo,
    store: &S,
    output_format: InputProcessableFormat,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    original_details: &Details,
    media: &crate::config::Media,
    hash: Hash,
) -> Result<(Details, Bytes), Error> {
    let guard = MetricsGuard::guard();
    let permit = crate::PROCESS_SEMAPHORE.acquire().await;

    let identifier = input_identifier(
        repo,
        store,
        output_format,
        hash.clone(),
        original_details,
        media,
    )
    .await?;

    let input_details = if let Some(details) = repo.details(&identifier).await? {
        details
    } else {
        let details = Details::from_store(store, &identifier, media.process_timeout).await?;

        repo.relate_details(&identifier, &details).await?;

        details
    };

    let input_format = input_details
        .internal_format()
        .processable_format()
        .expect("Already verified format is processable");

    let format = input_format
        .process_to(output_format)
        .ok_or(UploadError::InvalidProcessExtension)?;

    let quality = match format {
        ProcessableFormat::Image(format) => media.image.quality_for(format),
        ProcessableFormat::Animation(format) => media.animation.quality_for(format),
    };

    let mut processed_reader = crate::magick::process_image_store_read(
        store,
        &identifier,
        thumbnail_args,
        input_format,
        format,
        quality,
        media.process_timeout,
    )
    .await?;

    let mut vec = Vec::new();
    processed_reader
        .read_to_end(&mut vec)
        .instrument(tracing::info_span!("Reading processed image to vec"))
        .await?;
    let bytes = Bytes::from(vec);

    drop(permit);

    let details = Details::from_bytes(media.process_timeout, bytes.clone()).await?;

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
            let output_format = input_format
                .process_to(output_format)
                .ok_or(UploadError::InvalidProcessExtension)?;

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

            let reader = magick::thumbnail(
                store,
                &identifier,
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

        let motion_identifier = store.save_async_read(reader, media_type).await?;

        repo.relate_motion_identifier(hash, &motion_identifier)
            .await?;

        return Ok(motion_identifier);
    }

    repo.identifier(hash)
        .await?
        .ok_or(UploadError::MissingIdentifier)
        .map_err(From::from)
}
