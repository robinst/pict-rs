mod ffmpeg;
mod magick;

use crate::{
    concurrent_processor::ProcessMap,
    details::Details,
    error::{Error, UploadError},
    formats::{ImageFormat, InputProcessableFormat, InternalVideoFormat, ProcessableFormat},
    future::{WithMetrics, WithPollTimer, WithTimeout},
    repo::{Hash, VariantAlreadyExists},
    state::State,
    store::Store,
};

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
        metrics::counter!(crate::init_metrics::GENERATE_START).increment(1);
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
        metrics::histogram!(crate::init_metrics::GENERATE_DURATION, "completed" => (!self.armed).to_string())
            .record(self.start.elapsed().as_secs_f64());
        metrics::counter!(crate::init_metrics::GENERATE_END, "completed" => (!self.armed).to_string())
            .increment(1);
    }
}

#[tracing::instrument(skip(state, process_map, original_details, hash))]
pub(crate) async fn generate<S: Store + 'static>(
    state: &State<S>,
    process_map: &ProcessMap,
    format: InputProcessableFormat,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    original_details: &Details,
    hash: Hash,
) -> Result<(Details, Arc<str>), Error> {
    if state.config.server.danger_dummy_mode {
        let identifier = state
            .repo
            .identifier(hash)
            .await?
            .ok_or(UploadError::MissingIdentifier)?;

        Ok((original_details.clone(), identifier))
    } else {
        let process_fut = process(
            state,
            format,
            thumbnail_path.clone(),
            thumbnail_args,
            original_details,
            hash.clone(),
        )
        .with_poll_timer("process-future");

        let (details, identifier) = process_map
            .process(hash, thumbnail_path, process_fut)
            .with_poll_timer("process-map-future")
            .with_timeout(Duration::from_secs(state.config.media.process_timeout * 4))
            .with_metrics(crate::init_metrics::GENERATE_PROCESS)
            .await
            .map_err(|_| UploadError::ProcessTimeout)??;

        Ok((details, identifier))
    }
}

#[tracing::instrument(skip(state, hash))]
async fn process<S: Store + 'static>(
    state: &State<S>,
    output_format: InputProcessableFormat,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    original_details: &Details,
    hash: Hash,
) -> Result<(Details, Arc<str>), Error> {
    let guard = MetricsGuard::guard();
    let permit = crate::process_semaphore().acquire().await?;

    let identifier = input_identifier(state, output_format, hash.clone(), original_details).await?;

    let input_details = crate::ensure_details_identifier(state, &identifier).await?;

    let input_format = input_details
        .internal_format()
        .processable_format()
        .expect("Already verified format is processable");

    let format = input_format.process_to(output_format);

    let quality = match format {
        ProcessableFormat::Image(format) => state.config.media.image.quality_for(format),
        ProcessableFormat::Animation(format) => state.config.media.animation.quality_for(format),
    };

    let stream = state.store.to_stream(&identifier, None, None).await?;

    let bytes =
        crate::magick::process_image_command(state, thumbnail_args, input_format, format, quality)
            .await?
            .drive_with_stream(stream)
            .into_bytes_stream()
            .instrument(tracing::info_span!(
                "Reading processed image to BytesStream"
            ))
            .await?;

    drop(permit);

    let details = Details::from_bytes_stream(state, bytes.clone()).await?;

    let identifier = state
        .store
        .save_stream(
            bytes.into_io_stream(),
            details.media_type(),
            Some(details.file_extension()),
        )
        .await?;

    if let Err(VariantAlreadyExists) = state
        .repo
        .relate_variant_identifier(
            hash,
            thumbnail_path.to_string_lossy().to_string(),
            &identifier,
        )
        .await?
    {
        state.store.remove(&identifier).await?;
    }

    state.repo.relate_details(&identifier, &details).await?;

    guard.disarm();

    Ok((details, identifier)) as Result<(Details, Arc<str>), Error>
}

pub(crate) async fn ensure_motion_identifier<S>(
    state: &State<S>,
    hash: Hash,
    original_details: &Details,
) -> Result<Arc<str>, Error>
where
    S: Store + 'static,
{
    if let Some(identifier) = state.repo.motion_identifier(hash.clone()).await? {
        return Ok(identifier);
    };

    let identifier = state
        .repo
        .identifier(hash.clone())
        .await?
        .ok_or(UploadError::MissingIdentifier)?;

    let (reader, media_type, file_extension) =
        if let Some(processable_format) = original_details.internal_format().processable_format() {
            let thumbnail_format = state.config.media.image.format.unwrap_or(ImageFormat::Webp);

            let stream = state.store.to_stream(&identifier, None, None).await?;

            let process =
                magick::thumbnail_command(state, processable_format, thumbnail_format).await?;

            (
                process.drive_with_stream(stream),
                thumbnail_format.media_type(),
                thumbnail_format.file_extension(),
            )
        } else {
            let thumbnail_format = match state.config.media.image.format {
                Some(ImageFormat::Webp | ImageFormat::Avif | ImageFormat::Jxl) => {
                    ffmpeg::ThumbnailFormat::Webp
                }
                Some(ImageFormat::Png) => ffmpeg::ThumbnailFormat::Png,
                Some(ImageFormat::Jpeg) | None => ffmpeg::ThumbnailFormat::Jpeg,
            };

            let reader = ffmpeg::thumbnail(
                state,
                identifier,
                original_details
                    .video_format()
                    .unwrap_or(InternalVideoFormat::Mp4),
                thumbnail_format,
            )
            .await?;

            (
                reader,
                thumbnail_format.media_type(),
                thumbnail_format.file_extension(),
            )
        };

    let motion_identifier = reader
        .with_stdout(|stdout| async {
            state
                .store
                .save_async_read(stdout, media_type, Some(file_extension))
                .await
        })
        .await??;

    state
        .repo
        .relate_motion_identifier(hash, &motion_identifier)
        .await?;

    Ok(motion_identifier)
}

#[tracing::instrument(skip_all)]
async fn input_identifier<S>(
    state: &State<S>,
    output_format: InputProcessableFormat,
    hash: Hash,
    original_details: &Details,
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
        return ensure_motion_identifier(state, hash.clone(), original_details).await;
    }

    state
        .repo
        .identifier(hash)
        .await?
        .ok_or(UploadError::MissingIdentifier)
        .map_err(From::from)
}
