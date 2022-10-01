use crate::{
    config::ImageFormat,
    error::Error,
    ingest::Session,
    queue::{LocalBoxFuture, Process},
    repo::{Alias, DeleteToken, FullRepo, UploadId, UploadResult},
    serde_str::Serde,
    store::{Identifier, Store},
};
use futures_util::TryStreamExt;
use std::path::PathBuf;

pub(super) fn perform<'a, R, S>(
    repo: &'a R,
    store: &'a S,
    job: &'a [u8],
) -> LocalBoxFuture<'a, Result<(), Error>>
where
    R: FullRepo + 'static,
    S: Store + 'static,
{
    Box::pin(async move {
        match serde_json::from_slice(job) {
            Ok(job) => match job {
                Process::Ingest {
                    identifier,
                    upload_id,
                    declared_alias,
                    should_validate,
                    is_cached,
                } => {
                    process_ingest(
                        repo,
                        store,
                        identifier,
                        Serde::into_inner(upload_id),
                        declared_alias.map(Serde::into_inner),
                        should_validate,
                        is_cached,
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
                        target_format,
                        Serde::into_inner(source),
                        process_path,
                        process_args,
                    )
                    .await?
                }
            },
            Err(e) => {
                tracing::warn!("Invalid job: {}", format!("{}", e));
            }
        }

        Ok(())
    })
}

#[tracing::instrument(skip(repo, store))]
async fn process_ingest<R, S>(
    repo: &R,
    store: &S,
    unprocessed_identifier: Vec<u8>,
    upload_id: UploadId,
    declared_alias: Option<Alias>,
    should_validate: bool,
    is_cached: bool,
) -> Result<(), Error>
where
    R: FullRepo + 'static,
    S: Store,
{
    let fut = async {
        let unprocessed_identifier = S::Identifier::from_bytes(unprocessed_identifier)?;

        let stream = store
            .to_stream(&unprocessed_identifier, None, None)
            .await?
            .map_err(Error::from);

        let session = crate::ingest::ingest(
            repo,
            store,
            stream,
            declared_alias,
            should_validate,
            is_cached,
        )
        .await?;

        let token = session.delete_token().await?;

        store.remove(&unprocessed_identifier).await?;

        Ok((session, token)) as Result<(Session<R, S>, DeleteToken), Error>
    };

    let result = match fut.await {
        Ok((mut session, token)) => {
            let alias = session.alias().take().expect("Alias should exist").clone();
            let result = UploadResult::Success { alias, token };
            session.disarm();
            result
        }
        Err(e) => {
            tracing::warn!(
                "Failed to ingest {}, {}",
                format!("{}", e),
                format!("{:?}", e)
            );

            UploadResult::Failure {
                message: e.to_string(),
            }
        }
    };

    repo.complete(upload_id, result).await?;

    Ok(())
}

async fn generate<R: FullRepo, S: Store + 'static>(
    repo: &R,
    store: &S,
    target_format: ImageFormat,
    source: Alias,
    process_path: PathBuf,
    process_args: Vec<String>,
) -> Result<(), Error> {
    let hash = repo.hash(&source).await?;

    let path_string = process_path.to_string_lossy().to_string();
    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if identifier_opt.is_some() {
        return Ok(());
    }

    let original_details = crate::ensure_details(repo, store, &source).await?;

    crate::generate::generate(
        repo,
        store,
        target_format,
        source,
        process_path,
        process_args,
        original_details.to_input_format(),
        None,
        hash,
    )
    .await?;

    Ok(())
}
