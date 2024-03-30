use time::Instant;
use tracing::{Instrument, Span};

use crate::{
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    future::WithPollTimer,
    ingest::Session,
    queue::Process,
    repo::{Alias, UploadId, UploadResult},
    serde_str::Serde,
    state::State,
    store::Store,
    UploadQuery,
};
use std::{path::PathBuf, sync::Arc};

use super::{JobContext, JobFuture, JobResult};

pub(super) fn perform<'a, S>(state: &'a State<S>, job: serde_json::Value) -> JobFuture<'a>
where
    S: Store + 'static,
{
    Box::pin(async move {
        let job_text = format!("{job}");

        let job = serde_json::from_value(job)
            .map_err(|e| UploadError::InvalidJob(e, job_text))
            .abort()?;

        match job {
            Process::Ingest {
                identifier,
                upload_id,
                declared_alias,
                upload_query,
            } => {
                process_ingest(
                    state,
                    Arc::from(identifier),
                    Serde::into_inner(upload_id),
                    declared_alias.map(Serde::into_inner),
                    upload_query,
                )
                .with_poll_timer("process-ingest")
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
                    target_format,
                    Serde::into_inner(source),
                    process_path,
                    process_args,
                )
                .with_poll_timer("process-generate")
                .await?
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
        metrics::counter!(crate::init_metrics::BACKGROUND_UPLOAD_INGEST, "completed" => (!self.armed).to_string()).increment(1);
        metrics::histogram!(crate::init_metrics::BACKGROUND_UPLOAD_INGEST_DURATION, "completed" => (!self.armed).to_string()).record(self.start.elapsed().as_seconds_f64());

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
    upload_query: UploadQuery,
) -> JobResult
where
    S: Store + 'static,
{
    let guard = UploadGuard::guard(upload_id);

    let res = async {
        let ident = unprocessed_identifier.clone();
        let state2 = state.clone();

        let current_span = Span::current();
        let span = tracing::info_span!(parent: current_span, "error_boundary");

        let error_boundary = crate::sync::abort_on_drop(crate::sync::spawn(
            "ingest-media",
            async move {
                let stream =
                    crate::stream::from_err(state2.store.to_stream(&ident, None, None).await?);

                let session =
                    crate::ingest::ingest(&state2, stream, declared_alias, &upload_query).await?;

                Ok(session) as Result<Session, Error>
            }
            .instrument(span),
        ))
        .await;

        state.store.remove(&unprocessed_identifier).await?;

        error_boundary.map_err(|_| UploadError::Canceled)?
    }
    .await;

    let (result, err) = match res {
        Ok(session) => {
            let alias = session.alias().take().expect("Alias should exist").clone();
            let token = session.disarm();
            (UploadResult::Success { alias, token }, None)
        }
        Err(e) => (
            UploadResult::Failure {
                message: e.root_cause().to_string(),
                code: e.error_code().into_owned(),
            },
            Some(e),
        ),
    };

    state
        .repo
        .complete_upload(upload_id, result)
        .await
        .retry()?;

    if let Some(e) = err {
        return Err(e).abort();
    }

    guard.disarm();

    Ok(())
}

#[tracing::instrument(skip(state, process_path, process_args))]
async fn generate<S: Store + 'static>(
    state: &State<S>,
    target_format: InputProcessableFormat,
    source: Alias,
    process_path: PathBuf,
    process_args: Vec<String>,
) -> JobResult {
    let hash = state
        .repo
        .hash(&source)
        .await
        .retry()?
        .ok_or(UploadError::MissingAlias)
        .abort()?;

    let path_string = process_path.to_string_lossy().to_string();
    let identifier_opt = state
        .repo
        .variant_identifier(hash.clone(), path_string)
        .await
        .retry()?;

    if identifier_opt.is_some() {
        // don't generate already-generated variant
        return Ok(());
    }

    let original_details = crate::ensure_details(state, &source).await.retry()?;

    crate::generate::generate(
        state,
        target_format,
        process_path,
        process_args,
        &original_details,
        hash,
    )
    .await
    .abort()?;

    Ok(())
}
