use crate::{
    concurrent_processor::ProcessMap,
    config::Configuration,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    ingest::Session,
    queue::{Base64Bytes, LocalBoxFuture, Process},
    repo::{Alias, ArcRepo, UploadId, UploadResult},
    serde_str::Serde,
    store::{Identifier, Store},
    stream::StreamMap,
};
use std::path::PathBuf;

pub(super) fn perform<'a, S>(
    repo: &'a ArcRepo,
    store: &'a S,
    process_map: &'a ProcessMap,
    config: &'a Configuration,
    job: &'a [u8],
) -> LocalBoxFuture<'a, Result<(), Error>>
where
    S: Store + 'static,
{
    Box::pin(async move {
        match serde_json::from_slice(job) {
            Ok(job) => match job {
                Process::Ingest {
                    identifier: Base64Bytes(identifier),
                    upload_id,
                    declared_alias,
                } => {
                    process_ingest(
                        repo,
                        store,
                        identifier,
                        Serde::into_inner(upload_id),
                        declared_alias.map(Serde::into_inner),
                        &config.media,
                    )
                    .await?
                }
                Process::Generate {
                    target_format,
                    source,
                    process_path,
                    process_args,
                } => {
                    generate(
                        repo,
                        store,
                        process_map,
                        target_format,
                        Serde::into_inner(source),
                        process_path,
                        process_args,
                        config,
                    )
                    .await?
                }
            },
            Err(e) => {
                tracing::warn!("Invalid job: {}", format!("{e}"));
            }
        }

        Ok(())
    })
}

#[tracing::instrument(skip_all)]
async fn process_ingest<S>(
    repo: &ArcRepo,
    store: &S,
    unprocessed_identifier: Vec<u8>,
    upload_id: UploadId,
    declared_alias: Option<Alias>,
    media: &crate::config::Media,
) -> Result<(), Error>
where
    S: Store + 'static,
{
    let fut = async {
        let unprocessed_identifier = S::Identifier::from_bytes(unprocessed_identifier)?;

        let ident = unprocessed_identifier.clone();
        let store2 = store.clone();
        let repo = repo.clone();

        let media = media.clone();
        let error_boundary = actix_rt::spawn(async move {
            let stream = store2
                .to_stream(&ident, None, None)
                .await?
                .map(|res| res.map_err(Error::from));

            let session =
                crate::ingest::ingest(&repo, &store2, stream, declared_alias, &media).await?;

            Ok(session) as Result<Session<S>, Error>
        })
        .await;

        store.remove(&unprocessed_identifier).await?;

        error_boundary.map_err(|_| UploadError::Canceled)?
    };

    let result = match fut.await {
        Ok(session) => {
            let alias = session.alias().take().expect("Alias should exist").clone();
            let token = session.disarm();
            UploadResult::Success { alias, token }
        }
        Err(e) => {
            tracing::warn!("Failed to ingest\n{}\n{}", format!("{e}"), format!("{e:?}"));

            UploadResult::Failure {
                message: e.root_cause().to_string(),
            }
        }
    };

    repo.complete(upload_id, result).await?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all)]
async fn generate<S: Store + 'static>(
    repo: &ArcRepo,
    store: &S,
    process_map: &ProcessMap,
    target_format: InputProcessableFormat,
    source: Alias,
    process_path: PathBuf,
    process_args: Vec<String>,
    config: &Configuration,
) -> Result<(), Error> {
    let Some(hash) = repo.hash(&source).await? else {
        // Nothing to do
        return Ok(());
    };

    let path_string = process_path.to_string_lossy().to_string();
    let identifier_opt = repo.variant_identifier(hash.clone(), path_string).await?;

    if identifier_opt.is_some() {
        return Ok(());
    }

    let original_details = crate::ensure_details(repo, store, config, &source).await?;

    crate::generate::generate(
        repo,
        store,
        process_map,
        target_format,
        source,
        process_path,
        process_args,
        original_details.video_format(),
        None,
        &config.media,
        hash,
    )
    .await?;

    Ok(())
}
