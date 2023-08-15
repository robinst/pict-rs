use crate::{
    concurrent_processor::ProcessMap,
    config::Configuration,
    error::Error,
    formats::InputProcessableFormat,
    repo::{
        Alias, AliasRepo, DeleteToken, FullRepo, Hash, HashRepo, IdentifierRepo, JobId, QueueRepo,
        UploadId,
    },
    serde_str::Serde,
    store::{Identifier, Store},
};
use base64::{prelude::BASE64_STANDARD, Engine};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    time::{Duration, Instant},
};
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
        hash: Hash,
    },
    Identifier {
        identifier: Base64Bytes,
    },
    Alias {
        alias: Serde<Alias>,
        token: Serde<DeleteToken>,
    },
    Variant {
        hash: Hash,
        #[serde(skip_serializing_if = "Option::is_none")]
        variant: Option<String>,
    },
    AllVariants,
    OutdatedVariants,
    OutdatedProxies,
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

pub(crate) async fn cleanup_hash<R: QueueRepo>(repo: &R, hash: Hash) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Hash { hash })?;
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

async fn cleanup_variants<R: QueueRepo>(
    repo: &R,
    hash: Hash,
    variant: Option<String>,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Variant { hash, variant })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_outdated_proxies<R: QueueRepo>(repo: &R) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::OutdatedProxies)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_outdated_variants<R: QueueRepo>(repo: &R) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::OutdatedVariants)?;
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

pub(crate) async fn process_cleanup<R: FullRepo, S: Store>(
    repo: R,
    store: S,
    config: Configuration,
) {
    process_jobs(&repo, &store, &config, CLEANUP_QUEUE, cleanup::perform).await
}

pub(crate) async fn process_images<R: FullRepo + 'static, S: Store + 'static>(
    repo: R,
    store: S,
    process_map: ProcessMap,
    config: Configuration,
) {
    process_image_jobs(
        &repo,
        &store,
        &process_map,
        &config,
        PROCESS_QUEUE,
        process::perform,
    )
    .await
}

type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

async fn process_jobs<R, S, F>(
    repo: &R,
    store: &S,
    config: &Configuration,
    queue: &'static str,
    callback: F,
) where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    S: Store,
    for<'a> F: Fn(&'a R, &'a S, &'a Configuration, &'a [u8]) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    let worker_id = uuid::Uuid::new_v4();

    loop {
        let res = job_loop(repo, store, config, worker_id, queue, callback).await;

        if let Err(e) = res {
            tracing::warn!("Error processing jobs: {}", format!("{e}"));
            tracing::warn!("{}", format!("{e:?}"));
            continue;
        }

        break;
    }
}

struct MetricsGuard {
    worker_id: uuid::Uuid,
    queue: &'static str,
    start: Instant,
    armed: bool,
}

impl MetricsGuard {
    fn guard(worker_id: uuid::Uuid, queue: &'static str) -> Self {
        metrics::increment_counter!("pict-rs.job.start", "queue" => queue, "worker-id" => worker_id.to_string());

        Self {
            worker_id,
            queue,
            start: Instant::now(),
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for MetricsGuard {
    fn drop(&mut self) {
        metrics::histogram!("pict-rs.job.duration", self.start.elapsed().as_secs_f64(), "queue" => self.queue, "worker-id" => self.worker_id.to_string(), "completed" => (!self.armed).to_string());
        metrics::increment_counter!("pict-rs.job.end", "queue" => self.queue, "worker-id" => self.worker_id.to_string(), "completed" => (!self.armed).to_string());
    }
}

async fn job_loop<R, S, F>(
    repo: &R,
    store: &S,
    config: &Configuration,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    S: Store,
    for<'a> F: Fn(&'a R, &'a S, &'a Configuration, &'a [u8]) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    loop {
        let (job_id, bytes) = repo.pop(queue).await?;

        let span = tracing::info_span!("Running Job", worker_id = ?worker_id);

        let guard = MetricsGuard::guard(worker_id, queue);

        let res = span
            .in_scope(|| {
                heartbeat(
                    repo,
                    queue,
                    job_id,
                    (callback)(repo, store, config, bytes.as_ref()),
                )
            })
            .instrument(span)
            .await;

        repo.complete_job(queue, job_id).await?;

        res?;

        guard.disarm();
    }
}

async fn process_image_jobs<R, S, F>(
    repo: &R,
    store: &S,
    process_map: &ProcessMap,
    config: &Configuration,
    queue: &'static str,
    callback: F,
) where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    S: Store,
    for<'a> F: Fn(
            &'a R,
            &'a S,
            &'a ProcessMap,
            &'a Configuration,
            &'a [u8],
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    let worker_id = uuid::Uuid::new_v4();

    loop {
        let res =
            image_job_loop(repo, store, process_map, config, worker_id, queue, callback).await;

        if let Err(e) = res {
            tracing::warn!("Error processing jobs: {}", format!("{e}"));
            tracing::warn!("{}", format!("{e:?}"));
            continue;
        }

        break;
    }
}

async fn image_job_loop<R, S, F>(
    repo: &R,
    store: &S,
    process_map: &ProcessMap,
    config: &Configuration,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    S: Store,
    for<'a> F: Fn(
            &'a R,
            &'a S,
            &'a ProcessMap,
            &'a Configuration,
            &'a [u8],
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    loop {
        let (job_id, bytes) = repo.pop(queue).await?;

        let span = tracing::info_span!("Running Job", worker_id = ?worker_id);

        let guard = MetricsGuard::guard(worker_id, queue);

        let res = span
            .in_scope(|| {
                heartbeat(
                    repo,
                    queue,
                    job_id,
                    (callback)(repo, store, process_map, config, bytes.as_ref()),
                )
            })
            .instrument(span)
            .await;

        repo.complete_job(queue, job_id).await?;

        res?;

        guard.disarm();
    }
}

async fn heartbeat<R, Fut>(repo: &R, queue: &'static str, job_id: JobId, fut: Fut) -> Fut::Output
where
    R: QueueRepo,
    Fut: std::future::Future,
{
    let mut fut = std::pin::pin!(fut);

    let mut interval = actix_rt::time::interval(Duration::from_secs(5));

    let mut hb = None;

    loop {
        tokio::select! {
            output = &mut fut => {
                return output;
            }
            _ = interval.tick() => {
                if hb.is_none() {
                    hb = Some(repo.heartbeat(queue, job_id));
                }
            }
            opt = poll_opt(hb.as_mut()), if hb.is_some() => {
                if let Some(Err(e)) = opt {
                    tracing::warn!("Failed heartbeat\n{}", format!("{e:?}"));
                }
            }
        }
    }
}

async fn poll_opt<Fut>(opt: Option<&mut Fut>) -> Option<Fut::Output>
where
    Fut: std::future::Future + Unpin,
{
    match opt {
        None => None,
        Some(fut) => std::future::poll_fn(|cx| Pin::new(&mut *fut).poll(cx).map(Some)).await,
    }
}
