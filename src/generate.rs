use crate::{
    concurrent_processor::CancelSafeProcessor,
    config::ImageFormat,
    details::Details,
    error::{Error, UploadError},
    ffmpeg::{ThumbnailFormat, VideoFormat},
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
    format: ImageFormat,
    alias: Alias,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    input_format: Option<VideoFormat>,
    thumbnail_format: Option<ThumbnailFormat>,
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
    format: ImageFormat,
    alias: Alias,
    thumbnail_path: PathBuf,
    thumbnail_args: Vec<String>,
    input_format: Option<VideoFormat>,
    thumbnail_format: Option<ThumbnailFormat>,
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

        let reader = crate::ffmpeg::thumbnail(
            store.clone(),
            identifier,
            input_format.unwrap_or(VideoFormat::Mp4),
            thumbnail_format.unwrap_or(ThumbnailFormat::Jpeg),
        )
        .await?;
        let motion_identifier = store.save_async_read(reader).await?;

        repo.relate_motion_identifier(hash.clone(), &motion_identifier)
            .await?;

        motion_identifier
    };

    let mut processed_reader =
        crate::magick::process_image_store_read(store.clone(), identifier, thumbnail_args, format)?;

    let mut vec = Vec::new();
    processed_reader
        .read_to_end(&mut vec)
        .instrument(tracing::info_span!("Reading processed image to vec"))
        .await?;
    let bytes = Bytes::from(vec);

    drop(permit);

    let details = Details::from_bytes(bytes.clone(), format.as_hint()).await?;

    let identifier = store.save_bytes(bytes.clone()).await?;
    repo.relate_details(&identifier, &details).await?;
    repo.relate_variant_identifier(
        hash,
        thumbnail_path.to_string_lossy().to_string(),
        &identifier,
    )
    .await?;

    Ok((details, bytes)) as Result<(Details, Bytes), Error>
}
