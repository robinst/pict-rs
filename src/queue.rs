use crate::{
    config::ImageFormat,
    error::Error,
    repo::{Alias, AliasRepo, DeleteToken, FullRepo, HashRepo, IdentifierRepo, QueueRepo},
    serde_str::Serde,
    store::{Identifier, Store},
};
use std::{future::Future, path::PathBuf, pin::Pin};
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
    process_jobs(&repo, &store, worker_id, cleanup::perform).await
}

pub(crate) async fn process_images<R: FullRepo + 'static, S: Store>(
    repo: R,
    store: S,
    worker_id: String,
) {
    process_jobs(&repo, &store, worker_id, process::perform).await
}

type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

async fn process_jobs<R, S, F>(repo: &R, store: &S, worker_id: String, callback: F)
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
    for<'a> F: Fn(&'a R, &'a S, &'a [u8]) -> LocalBoxFuture<'a, Result<(), Error>> + Copy,
{
    if let Ok(Some(job)) = repo.in_progress(worker_id.as_bytes().to_vec()).await {
        if let Err(e) = (callback)(repo, store, job.as_ref()).await {
            tracing::warn!("Failed to run previously dropped job: {}", e);
            tracing::warn!("{:?}", e);
        }
    }
    loop {
        let res = job_loop(repo, store, worker_id.clone(), callback).await;

        if let Err(e) = res {
            tracing::warn!("Error processing jobs: {}", e);
            tracing::warn!("{:?}", e);
            continue;
        }

        break;
    }
}

async fn job_loop<R, S, F>(repo: &R, store: &S, worker_id: String, callback: F) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
    for<'a> F: Fn(&'a R, &'a S, &'a [u8]) -> LocalBoxFuture<'a, Result<(), Error>> + Copy,
{
    loop {
        let bytes = repo
            .pop(CLEANUP_QUEUE, worker_id.as_bytes().to_vec())
            .await?;

        (callback)(repo, store, bytes.as_ref()).await?;
    }
}
