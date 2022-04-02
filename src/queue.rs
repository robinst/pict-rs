use crate::{
    config::ImageFormat,
    error::Error,
    repo::{Alias, AliasRepo, DeleteToken, FullRepo, HashRepo, IdentifierRepo, QueueRepo},
    serde_str::Serde,
    store::{Identifier, Store},
};
use std::{future::Future, path::PathBuf, pin::Pin};
use tracing::Instrument;
use uuid::Uuid;

mod cleanup;
mod process;

const CLEANUP_QUEUE: &str = "cleanup";
const PROCESS_QUEUE: &str = "process";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Cleanup {
    Hash {
        hash: Vec<u8>,
    },
    Identifier {
        identifier: Vec<u8>,
    },
    Alias {
        alias: Serde<Alias>,
        token: Serde<DeleteToken>,
    },
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Process {
    Ingest {
        identifier: Vec<u8>,
        upload_id: Uuid,
        declared_alias: Option<Serde<Alias>>,
        should_validate: bool,
    },
    Generate {
        target_format: ImageFormat,
        source: Serde<Alias>,
        process_path: PathBuf,
        process_args: Vec<String>,
    },
}

pub(crate) async fn cleanup_alias<R: QueueRepo>(
    repo: &R,
    alias: Alias,
    token: DeleteToken,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Alias {
        alias: Serde::new(alias),
        token: Serde::new(token),
    })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_hash<R: QueueRepo>(repo: &R, hash: R::Bytes) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Hash {
        hash: hash.as_ref().to_vec(),
    })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_identifier<R: QueueRepo, I: Identifier>(
    repo: &R,
    identifier: I,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Identifier {
        identifier: identifier.to_bytes()?,
    })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn queue_ingest<R: QueueRepo>(
    repo: &R,
    identifier: Vec<u8>,
    upload_id: Uuid,
    declared_alias: Option<Alias>,
    should_validate: bool,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Process::Ingest {
        identifier,
        declared_alias: declared_alias.map(Serde::new),
        upload_id,
        should_validate,
    })?;
    repo.push(PROCESS_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn queue_generate<R: QueueRepo>(
    repo: &R,
    target_format: ImageFormat,
    source: Alias,
    process_path: PathBuf,
    process_args: Vec<String>,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Process::Generate {
        target_format,
        source: Serde::new(source),
        process_path,
        process_args,
    })?;
    repo.push(PROCESS_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn process_cleanup<R: FullRepo, S: Store>(repo: R, store: S, worker_id: String) {
    process_jobs(&repo, &store, worker_id, CLEANUP_QUEUE, cleanup::perform).await
}

pub(crate) async fn process_images<R: FullRepo + 'static, S: Store + 'static>(
    repo: R,
    store: S,
    worker_id: String,
) {
    process_jobs(&repo, &store, worker_id, PROCESS_QUEUE, process::perform).await
}

type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

async fn process_jobs<R, S, F>(
    repo: &R,
    store: &S,
    worker_id: String,
    queue: &'static str,
    callback: F,
) where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
    for<'a> F: Fn(&'a R, &'a S, &'a [u8]) -> LocalBoxFuture<'a, Result<(), Error>> + Copy,
{
    loop {
        let res = job_loop(repo, store, worker_id.clone(), queue, callback).await;

        if let Err(e) = res {
            tracing::warn!("Error processing jobs: {}", e);
            tracing::warn!("{:?}", e);
            continue;
        }

        break;
    }
}

async fn job_loop<R, S, F>(
    repo: &R,
    store: &S,
    worker_id: String,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
    for<'a> F: Fn(&'a R, &'a S, &'a [u8]) -> LocalBoxFuture<'a, Result<(), Error>> + Copy,
{
    loop {
        let bytes = repo.pop(queue, worker_id.as_bytes().to_vec()).await?;

        let span = tracing::info_span!("Running Job", worker_id = ?worker_id);

        span.in_scope(|| (callback)(repo, store, bytes.as_ref()))
            .instrument(span)
            .await?;
    }
}
