use crate::{
    concurrent_processor::CancelSafeProcessor,
    details::Details,
    error::{Error, UploadError},
    ffmpeg::ThumbnailFormat,
    formats::{InputProcessableFormat, InternalVideoFormat},
    repo::{Alias, FullRepo},
    store::Store,
};
use actix_web::web::Bytes;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;
use tracing::Instrument;

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(repo, store, hash))]
pub(crate) async fn generate<R: FullRepo, S: Store + 'static>(
    repo: &R,
    store: &S,
    format: InputProcessableFormat,
    alias: Alias,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    input_format: Option<InternalVideoFormat>,
    thumbnail_format: Option<ThumbnailFormat>,
    media: &'static crate::config::Media,
    hash: R::Bytes,
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

    let (details, bytes) =
        CancelSafeProcessor::new(hash.as_ref(), thumbnail_path, process_fut).await?;

    Ok((details, bytes))
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(repo, store, hash))]
async fn process<R: FullRepo, S: Store + 'static>(
    repo: &R,
    store: &S,
    output_format: InputProcessableFormat,
    alias: Alias,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    input_format: Option<InternalVideoFormat>,
    thumbnail_format: Option<ThumbnailFormat>,
    media: &'static crate::config::Media,
    hash: R::Bytes,
) -> Result<(Details, Bytes), Error> {
    let permit = crate::PROCESS_SEMAPHORE.acquire().await;

    let identifier = if let Some(identifier) = repo
        .still_identifier_from_alias::<S::Identifier>(&alias)
        .await?
    {
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
        let details = Details::from_store(store, &identifier).await?;

        repo.relate_details(&identifier, &details).await?;

        details
    };

    let input_format = input_details
        .internal_format()
        .and_then(|format| format.processable_format())
        .expect("Valid details should always have internal format");

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
    )
    .await?;

    let mut vec = Vec::new();
    processed_reader
        .read_to_end(&mut vec)
        .instrument(tracing::info_span!("Reading processed image to vec"))
        .await?;
    let bytes = Bytes::from(vec);

    drop(permit);

    let details = Details::from_bytes(bytes.clone()).await?;

    let identifier = store
        .save_bytes(bytes.clone(), details.media_type())
        .await?;
    repo.relate_details(&identifier, &details).await?;
    repo.relate_variant_identifier(
        hash,
        thumbnail_path.to_string_lossy().to_string(),
        &identifier,
    )
    .await?;

    Ok((details, bytes)) as Result<(Details, Bytes), Error>
}
