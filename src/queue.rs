use crate::{
    concurrent_processor::ProcessMap,
    config::Configuration,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    repo::{Alias, DeleteToken, FullRepo, Hash, JobId, UploadId},
    serde_str::Serde,
    store::{Identifier, Store},
};
use base64::{prelude::BASE64_STANDARD, Engine};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
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

pub(crate) async fn cleanup_alias(
    repo: &Arc<dyn FullRepo>,
    alias: Alias,
    token: DeleteToken,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Alias {
        alias: Serde::new(alias),
        token: Serde::new(token),
    })
    .map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_hash(repo: &Arc<dyn FullRepo>, hash: Hash) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Hash { hash }).map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_identifier<I: Identifier>(
    repo: &Arc<dyn FullRepo>,
    identifier: I,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::Identifier {
        identifier: Base64Bytes(identifier.to_bytes()?),
    })
    .map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

async fn cleanup_variants(
    repo: &Arc<dyn FullRepo>,
    hash: Hash,
    variant: Option<String>,
) -> Result<(), Error> {
    let job =
        serde_json::to_vec(&Cleanup::Variant { hash, variant }).map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_outdated_proxies(repo: &Arc<dyn FullRepo>) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::OutdatedProxies).map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_outdated_variants(repo: &Arc<dyn FullRepo>) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::OutdatedVariants).map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn cleanup_all_variants(repo: &Arc<dyn FullRepo>) -> Result<(), Error> {
    let job = serde_json::to_vec(&Cleanup::AllVariants).map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn queue_ingest(
    repo: &Arc<dyn FullRepo>,
    identifier: Vec<u8>,
    upload_id: UploadId,
    declared_alias: Option<Alias>,
) -> Result<(), Error> {
    let job = serde_json::to_vec(&Process::Ingest {
        identifier: Base64Bytes(identifier),
        declared_alias: declared_alias.map(Serde::new),
        upload_id: Serde::new(upload_id),
    })
    .map_err(UploadError::PushJob)?;
    repo.push(PROCESS_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn queue_generate(
    repo: &Arc<dyn FullRepo>,
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
    })
    .map_err(UploadError::PushJob)?;
    repo.push(PROCESS_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn process_cleanup<S: Store>(
    repo: Arc<dyn FullRepo>,
    store: S,
    config: Configuration,
) {
    process_jobs(&repo, &store, &config, CLEANUP_QUEUE, cleanup::perform).await
}

pub(crate) async fn process_images<S: Store + 'static>(
    repo: Arc<dyn FullRepo>,
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

async fn process_jobs<S, F>(
    repo: &Arc<dyn FullRepo>,
    store: &S,
    config: &Configuration,
    queue: &'static str,
    callback: F,
) where
    S: Store,
    for<'a> F: Fn(
            &'a Arc<dyn FullRepo>,
            &'a S,
            &'a Configuration,
            &'a [u8],
        ) -> LocalBoxFuture<'a, Result<(), Error>>
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

async fn job_loop<S, F>(
    repo: &Arc<dyn FullRepo>,
    store: &S,
    config: &Configuration,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    S: Store,
    for<'a> F: Fn(
            &'a Arc<dyn FullRepo>,
            &'a S,
            &'a Configuration,
            &'a [u8],
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    loop {
        let fut = async {
            let (job_id, bytes) = repo.pop(queue, worker_id).await?;

            let span = tracing::info_span!("Running Job");

            let guard = MetricsGuard::guard(worker_id, queue);

            let res = span
                .in_scope(|| {
                    heartbeat(
                        repo,
                        queue,
                        worker_id,
                        job_id,
                        (callback)(repo, store, config, bytes.as_ref()),
                    )
                })
                .instrument(span)
                .await;

            repo.complete_job(queue, worker_id, job_id).await?;

            res?;

            guard.disarm();

            Ok(()) as Result<(), Error>
        };

        fut.instrument(tracing::info_span!("tick", worker_id = %worker_id))
            .await?;
    }
}

async fn process_image_jobs<S, F>(
    repo: &Arc<dyn FullRepo>,
    store: &S,
    process_map: &ProcessMap,
    config: &Configuration,
    queue: &'static str,
    callback: F,
) where
    S: Store,
    for<'a> F: Fn(
            &'a Arc<dyn FullRepo>,
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

async fn image_job_loop<S, F>(
    repo: &Arc<dyn FullRepo>,
    store: &S,
    process_map: &ProcessMap,
    config: &Configuration,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    S: Store,
    for<'a> F: Fn(
            &'a Arc<dyn FullRepo>,
            &'a S,
            &'a ProcessMap,
            &'a Configuration,
            &'a [u8],
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    loop {
        let fut = async {
            let (job_id, bytes) = repo.pop(queue, worker_id).await?;

            let span = tracing::info_span!("Running Job");

            let guard = MetricsGuard::guard(worker_id, queue);

            let res = span
                .in_scope(|| {
                    heartbeat(
                        repo,
                        queue,
                        worker_id,
                        job_id,
                        (callback)(repo, store, process_map, config, bytes.as_ref()),
                    )
                })
                .instrument(span)
                .await;

            repo.complete_job(queue, worker_id, job_id).await?;

            res?;

            guard.disarm();
            Ok(()) as Result<(), Error>
        };

        fut.instrument(tracing::info_span!("tick", worker_id = %worker_id))
            .await?;
    }
}

async fn heartbeat<Fut>(
    repo: &Arc<dyn FullRepo>,
    queue: &'static str,
    worker_id: uuid::Uuid,
    job_id: JobId,
    fut: Fut,
) -> Fut::Output
where
    Fut: std::future::Future,
{
    let mut fut =
        std::pin::pin!(fut.instrument(tracing::info_span!("job-future", job_id = ?job_id)));

    let mut interval = actix_rt::time::interval(Duration::from_secs(5));

    let mut hb = None;

    loop {
        tokio::select! {
            output = &mut fut => {
                return output;
            }
            _ = interval.tick() => {
                if hb.is_none() {
                    hb = Some(repo.heartbeat(queue, worker_id, job_id));
                }
            }
            opt = poll_opt(hb.as_mut()), if hb.is_some() => {
                hb.take();

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
        Some(fut) => Some(fut.await),
    }
}
