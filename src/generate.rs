use crate::{
    concurrent_processor::ProcessMap,
    details::Details,
    error::{Error, UploadError},
    ffmpeg::ThumbnailFormat,
    formats::{InputProcessableFormat, InternalVideoFormat},
    repo::{Alias, ArcRepo, Hash, VariantAlreadyExists},
    store::Store,
};
use actix_web::web::Bytes;
use std::{path::PathBuf, time::Instant};
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
    alias: Alias,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    input_format: Option<InternalVideoFormat>,
    thumbnail_format: Option<ThumbnailFormat>,
    media: &crate::config::Media,
    hash: Hash,
) -> Result<(Details, Bytes), Error> {
    let process_fut = process(
        repo,
        store,
        format,
        alias,
        thumbnail_path.clone(),
        thumbnail_args,
        input_format,
        thumbnail_format,
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
    alias: Alias,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    input_format: Option<InternalVideoFormat>,
    thumbnail_format: Option<ThumbnailFormat>,
    media: &crate::config::Media,
    hash: Hash,
) -> Result<(Details, Bytes), Error> {
    let guard = MetricsGuard::guard();
    let permit = crate::PROCESS_SEMAPHORE.acquire().await;

    let identifier = if let Some(identifier) = repo.still_identifier_from_alias(&alias).await? {
        identifier
    } else {
        let Some(identifier) = repo.identifier(hash.clone()).await? else {
            return Err(UploadError::MissingIdentifier.into());
        };

        let thumbnail_format = thumbnail_format.unwrap_or(ThumbnailFormat::Jpeg);

        let reader = crate::ffmpeg::thumbnail(
            store.clone(),
            identifier,
            input_format.unwrap_or(InternalVideoFormat::Mp4),
            thumbnail_format,
            media.process_timeout,
        )
        .await?;

        let motion_identifier = store
            .save_async_read(reader, thumbnail_format.media_type())
            .await?;

        repo.relate_motion_identifier(hash.clone(), &motion_identifier)
            .await?;

        motion_identifier
    };

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

    let Some(format) = input_format.process_to(output_format) else {
        return Err(UploadError::InvalidProcessExtension.into());
    };

    let quality = match format {
        crate::formats::ProcessableFormat::Image(format) => media.image.quality_for(format),
        crate::formats::ProcessableFormat::Animation(format) => media.animation.quality_for(format),
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
