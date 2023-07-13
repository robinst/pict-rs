use crate::{
    error::Error,
    formats::InputProcessableFormat,
    repo::{
        Alias, AliasRepo, DeleteToken, FullRepo, HashRepo, IdentifierRepo, QueueRepo, UploadId,
    },
    serde_str::Serde,
    store::{Identifier, Store},
};
use base64::{prelude::BASE64_STANDARD, Engine};
use std::{future::Future, path::PathBuf, pin::Pin};
use tracing::Instrument;

mod cleanup;
mod process;

#[derive(Debug)]
struct Base64Bytes(Vec<u8>);

impl serde::Serialize for Base64Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = BASE64_STANDARD.encode(&self.0);
        s.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Base64Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = serde::Deserialize::deserialize(deserializer)?;
        BASE64_STANDARD
            .decode(s)
            .map(Base64Bytes)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

const CLEANUP_QUEUE: &str = "cleanup";
const PROCESS_QUEUE: &str = "process";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Cleanup {
    Hash {
        hash: Base64Bytes,
    },
    Identifier {
        identifier: Base64Bytes,
    },
    Alias {
        alias: Serde<Alias>,
        token: Serde<DeleteToken>,
    },
    Variant {
        hash: Base64Bytes,
    },
    AllVariants,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Process {
    Ingest {
        identifier: Base64Bytes,
        upload_id: Serde<UploadId>,
        declared_alias: Option<Serde<Alias>>,
    },
    Generate {
        target_format: InputProcessableFormat,
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
        hash: Base64Bytes(hash.as_ref().to_vec()),
    })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_identifier<R: QueueRepo, I: Identifier>(
    repo: &R,
    identifier: I,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Identifier {
        identifier: Base64Bytes(identifier.to_bytes()?),
    })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

async fn cleanup_variants<R: QueueRepo>(repo: &R, hash: R::Bytes) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Variant {
        hash: Base64Bytes(hash.as_ref().to_vec()),
    })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_all_variants<R: QueueRepo>(repo: &R) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::AllVariants)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn queue_ingest<R: QueueRepo>(
    repo: &R,
    identifier: Vec<u8>,
    upload_id: UploadId,
    declared_alias: Option<Alias>,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Process::Ingest {
        identifier: Base64Bytes(identifier),
        declared_alias: declared_alias.map(Serde::new),
        upload_id: Serde::new(upload_id),
    })?;
    repo.push(PROCESS_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn queue_generate<R: QueueRepo>(
    repo: &R,
    target_format: InputProcessableFormat,
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
            tracing::warn!("Error processing jobs: {}", format!("{e}"));
            tracing::warn!("{}", format!("{e:?}"));
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
