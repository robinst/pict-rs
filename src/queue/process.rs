use reqwest_middleware::ClientWithMiddleware;
use time::Instant;

use crate::{
    concurrent_processor::ProcessMap,
    config::Configuration,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    future::LocalBoxFuture,
    ingest::Session,
    queue::Process,
    repo::{Alias, ArcRepo, UploadId, UploadResult},
    serde_str::Serde,
    store::Store,
};
use std::{path::PathBuf, sync::Arc};

pub(super) fn perform<'a, S>(
    repo: &'a ArcRepo,
    store: &'a S,
    client: &'a ClientWithMiddleware,
    process_map: &'a ProcessMap,
    config: &'a Configuration,
    job: serde_json::Value,
) -> LocalBoxFuture<'a, Result<(), Error>>
where
    S: Store + 'static,
{
    Box::pin(async move {
        match serde_json::from_value(job) {
            Ok(job) => match job {
                Process::Ingest {
                    identifier,
                    upload_id,
                    declared_alias,
                } => {
                    process_ingest(
                        repo,
                        store,
                        client,
                        Arc::from(identifier),
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

struct UploadGuard {
    armed: bool,
    start: Instant,
    upload_id: UploadId,
}

impl UploadGuard {
    fn guard(upload_id: UploadId) -> Self {
        Self {
            armed: true,
            start: Instant::now(),
            upload_id,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for UploadGuard {
    fn drop(&mut self) {
        metrics::increment_counter!("pict-rs.background.upload.ingest", "completed" => (!self.armed).to_string());
        metrics::histogram!("pict-rs.background.upload.ingest.duration", self.start.elapsed().as_seconds_f64(), "completed" => (!self.armed).to_string());

        if self.armed {
            tracing::warn!(
                "Upload future for {} dropped before completion! This can cause clients to wait forever",
                self.upload_id,
            );
        }
    }
}

#[tracing::instrument(skip(repo, store, client, media))]
async fn process_ingest<S>(
    repo: &ArcRepo,
    store: &S,
    client: &ClientWithMiddleware,
    unprocessed_identifier: Arc<str>,
    upload_id: UploadId,
    declared_alias: Option<Alias>,
    media: &crate::config::Media,
) -> Result<(), Error>
where
    S: Store + 'static,
{
    let guard = UploadGuard::guard(upload_id);

    let fut = async {
        let ident = unprocessed_identifier.clone();
        let store2 = store.clone();
        let repo = repo.clone();
        let client = client.clone();

        let media = media.clone();
        let error_boundary = crate::sync::spawn(async move {
            let stream = crate::stream::from_err(store2.to_stream(&ident, None, None).await?);

            let session =
                crate::ingest::ingest(&repo, &store2, &client, stream, declared_alias, &media)
                    .await?;

            Ok(session) as Result<Session, Error>
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
                code: e.error_code().into_owned(),
            }
        }
    };

    repo.complete_upload(upload_id, result).await?;

    guard.disarm();

    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip(repo, store, process_map, process_path, process_args, config))]
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
        process_path,
        process_args,
        &original_details,
        &config.media,
        hash,
    )
    .await?;

    Ok(())
}
