use time::Instant;
use tracing::{Instrument, Span};

use crate::{
    concurrent_processor::ProcessMap,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    future::LocalBoxFuture,
    ingest::Session,
    queue::Process,
    repo::{Alias, UploadId, UploadResult},
    serde_str::Serde,
    state::State,
    store::Store,
};
use std::{path::PathBuf, sync::Arc};

pub(super) fn perform<'a, S>(
    state: &'a State<S>,
    process_map: &'a ProcessMap,
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
                        state,
                        Arc::from(identifier),
                        Serde::into_inner(upload_id),
                        declared_alias.map(Serde::into_inner),
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
                        state,
                        process_map,
                        target_format,
                        Serde::into_inner(source),
                        process_path,
                        process_args,
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
        metrics::counter!("pict-rs.background.upload.ingest", "completed" => (!self.armed).to_string()).increment(1);
        metrics::histogram!("pict-rs.background.upload.ingest.duration", "completed" => (!self.armed).to_string()).record(self.start.elapsed().as_seconds_f64());

        if self.armed {
            tracing::warn!(
                "Upload future for {} dropped before completion! This can cause clients to wait forever",
                self.upload_id,
            );
        }
    }
}

#[tracing::instrument(skip(state))]
async fn process_ingest<S>(
    state: &State<S>,
    unprocessed_identifier: Arc<str>,
    upload_id: UploadId,
    declared_alias: Option<Alias>,
) -> Result<(), Error>
where
    S: Store + 'static,
{
    let guard = UploadGuard::guard(upload_id);

    let fut = async {
        let ident = unprocessed_identifier.clone();
        let state2 = state.clone();

        let current_span = Span::current();
        let span = tracing::info_span!(parent: current_span, "error_boundary");

        let error_boundary = crate::sync::abort_on_drop(crate::sync::spawn(
            "ingest-media",
            async move {
                let stream =
                    crate::stream::from_err(state2.store.to_stream(&ident, None, None).await?);

                let session = crate::ingest::ingest(&state2, stream, declared_alias).await?;

                Ok(session) as Result<Session, Error>
            }
            .instrument(span),
        ))
        .await;

        state.store.remove(&unprocessed_identifier).await?;

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

    state.repo.complete_upload(upload_id, result).await?;

    guard.disarm();

    Ok(())
}

#[tracing::instrument(skip(state, process_map, process_path, process_args))]
async fn generate<S: Store + 'static>(
    state: &State<S>,
    process_map: &ProcessMap,
    target_format: InputProcessableFormat,
    source: Alias,
    process_path: PathBuf,
    process_args: Vec<String>,
) -> Result<(), Error> {
    let Some(hash) = state.repo.hash(&source).await? else {
        // Nothing to do
        return Ok(());
    };

    let path_string = process_path.to_string_lossy().to_string();
    let identifier_opt = state
        .repo
        .variant_identifier(hash.clone(), path_string)
        .await?;

    if identifier_opt.is_some() {
        return Ok(());
    }

    let original_details = crate::ensure_details(state, &source).await?;

    crate::generate::generate(
        state,
        process_map,
        target_format,
        process_path,
        process_args,
        &original_details,
        hash,
    )
    .await?;

    Ok(())
}
