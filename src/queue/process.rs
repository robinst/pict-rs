use crate::{
    config::ImageFormat,
    error::Error,
    queue::{LocalBoxFuture, Process},
    repo::{Alias, AliasRepo, HashRepo, IdentifierRepo, QueueRepo},
    serde_str::Serde,
    store::Store,
};
use std::path::PathBuf;
use uuid::Uuid;

pub(super) fn perform<'a, R, S>(
    repo: &'a R,
    store: &'a S,
    job: &'a [u8],
) -> LocalBoxFuture<'a, Result<(), Error>>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
{
    Box::pin(async move {
        match serde_json::from_slice(job) {
            Ok(job) => match job {
                Process::Ingest {
                    identifier,
                    upload_id,
                    declared_alias,
                    should_validate,
                } => {
                    ingest(
                        repo,
                        store,
                        identifier,
                        upload_id,
                        declared_alias.map(Serde::into_inner),
                        should_validate,
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
                tracing::warn!("Invalid job: {}", e);
            }
        }

        Ok(())
    })
}

async fn ingest<R, S>(
    repo: &R,
    store: &S,
    identifier: Vec<u8>,
    upload_id: Uuid,
    declared_alias: Option<Alias>,
    should_validate: bool,
) -> Result<(), Error> {
    unimplemented!("do this")
}

async fn generate<R, S>(
    repo: &R,
    store: &S,
    target_format: ImageFormat,
    source: Alias,
    process_path: PathBuf,
    process_args: Vec<String>,
) -> Result<(), Error> {
    unimplemented!("do this")
}
