use crate::{
    concurrent_processor::ProcessMap,
    config::Configuration,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    future::LocalBoxFuture,
    repo::{Alias, ArcRepo, DeleteToken, Hash, JobId, UploadId},
    serde_str::Serde,
    store::Store,
    tmp_file::ArcTmpDir,
};
use reqwest_middleware::ClientWithMiddleware;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::Instrument;

pub(crate) mod cleanup;
mod process;

const CLEANUP_QUEUE: &str = "cleanup";
const PROCESS_QUEUE: &str = "process";
const OUTDATED_PROXIES_UNIQUE_KEY: &str = "outdated-proxies";
const OUTDATED_VARIANTS_UNIQUE_KEY: &str = "outdated-variants";
const ALL_VARIANTS_UNIQUE_KEY: &str = "all-variants";
const PRUNE_MISSING_UNIQUE_KEY: &str = "prune-missing";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Cleanup {
    Hash {
        hash: Hash,
    },
    Identifier {
        identifier: String,
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
    Prune,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Process {
    Ingest {
        identifier: String,
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
    repo: &ArcRepo,
    alias: Alias,
    token: DeleteToken,
) -> Result<(), Error> {
    let job = serde_json::to_value(Cleanup::Alias {
        alias: Serde::new(alias),
        token: Serde::new(token),
    })
    .map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job, None).await?;
    Ok(())
}

pub(crate) async fn cleanup_hash(repo: &ArcRepo, hash: Hash) -> Result<(), Error> {
    let job = serde_json::to_value(Cleanup::Hash { hash }).map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job, None).await?;
    Ok(())
}

pub(crate) async fn cleanup_identifier(repo: &ArcRepo, identifier: &Arc<str>) -> Result<(), Error> {
    let job = serde_json::to_value(Cleanup::Identifier {
        identifier: identifier.to_string(),
    })
    .map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job, None).await?;
    Ok(())
}

async fn cleanup_variants(
    repo: &ArcRepo,
    hash: Hash,
    variant: Option<String>,
) -> Result<(), Error> {
    let job =
        serde_json::to_value(Cleanup::Variant { hash, variant }).map_err(UploadError::PushJob)?;
    repo.push(CLEANUP_QUEUE, job, None).await?;
    Ok(())
}

pub(crate) async fn cleanup_outdated_proxies(repo: &ArcRepo) -> Result<(), Error> {
    let job = serde_json::to_value(Cleanup::OutdatedProxies).map_err(UploadError::PushJob)?;
    if repo
        .push(CLEANUP_QUEUE, job, Some(OUTDATED_PROXIES_UNIQUE_KEY))
        .await?
        .is_none()
    {
        tracing::debug!("outdated proxies conflict");
    }
    Ok(())
}

pub(crate) async fn cleanup_outdated_variants(repo: &ArcRepo) -> Result<(), Error> {
    let job = serde_json::to_value(Cleanup::OutdatedVariants).map_err(UploadError::PushJob)?;
    if repo
        .push(CLEANUP_QUEUE, job, Some(OUTDATED_VARIANTS_UNIQUE_KEY))
        .await?
        .is_none()
    {
        tracing::debug!("outdated variants conflict");
    }
    Ok(())
}

pub(crate) async fn cleanup_all_variants(repo: &ArcRepo) -> Result<(), Error> {
    let job = serde_json::to_value(Cleanup::AllVariants).map_err(UploadError::PushJob)?;
    if repo
        .push(CLEANUP_QUEUE, job, Some(ALL_VARIANTS_UNIQUE_KEY))
        .await?
        .is_none()
    {
        tracing::debug!("all variants conflict");
    }
    Ok(())
}

pub(crate) async fn prune_missing(repo: &ArcRepo) -> Result<(), Error> {
    let job = serde_json::to_value(Cleanup::Prune).map_err(UploadError::PushJob)?;
    if repo
        .push(CLEANUP_QUEUE, job, Some(PRUNE_MISSING_UNIQUE_KEY))
        .await?
        .is_none()
    {
        tracing::debug!("prune missing conflict");
    }
    Ok(())
}

pub(crate) async fn queue_ingest(
    repo: &ArcRepo,
    identifier: &Arc<str>,
    upload_id: UploadId,
    declared_alias: Option<Alias>,
) -> Result<(), Error> {
    let job = serde_json::to_value(Process::Ingest {
        identifier: identifier.to_string(),
        declared_alias: declared_alias.map(Serde::new),
        upload_id: Serde::new(upload_id),
    })
    .map_err(UploadError::PushJob)?;
    repo.push(PROCESS_QUEUE, job, None).await?;
    Ok(())
}

pub(crate) async fn queue_generate(
    repo: &ArcRepo,
    target_format: InputProcessableFormat,
    source: Alias,
    process_path: PathBuf,
    process_args: Vec<String>,
) -> Result<(), Error> {
    let job = serde_json::to_value(Process::Generate {
        target_format,
        source: Serde::new(source),
        process_path,
        process_args,
    })
    .map_err(UploadError::PushJob)?;
    repo.push(PROCESS_QUEUE, job, None).await?;
    Ok(())
}

pub(crate) async fn process_cleanup<S: Store + 'static>(
    repo: ArcRepo,
    store: S,
    config: Configuration,
) {
    process_jobs(&repo, &store, &config, CLEANUP_QUEUE, cleanup::perform).await
}

pub(crate) async fn process_images<S: Store + 'static>(
    tmp_dir: ArcTmpDir,
    repo: ArcRepo,
    store: S,
    client: ClientWithMiddleware,
    process_map: ProcessMap,
    config: Configuration,
) {
    process_image_jobs(
        &tmp_dir,
        &repo,
        &store,
        &client,
        &process_map,
        &config,
        PROCESS_QUEUE,
        process::perform,
    )
    .await
}

struct MetricsGuard {
    worker_id: uuid::Uuid,
    queue: &'static str,
    start: Instant,
    armed: bool,
}

impl MetricsGuard {
    fn guard(worker_id: uuid::Uuid, queue: &'static str) -> Self {
        metrics::counter!("pict-rs.job.start", "queue" => queue, "worker-id" => worker_id.to_string()).increment(1);

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
        metrics::histogram!("pict-rs.job.duration", "queue" => self.queue, "worker-id" => self.worker_id.to_string(), "completed" => (!self.armed).to_string()).record(self.start.elapsed().as_secs_f64());
        metrics::counter!("pict-rs.job.end", "queue" => self.queue, "worker-id" => self.worker_id.to_string(), "completed" => (!self.armed).to_string()).increment(1);
    }
}

async fn process_jobs<S, F>(
    repo: &ArcRepo,
    store: &S,
    config: &Configuration,
    queue: &'static str,
    callback: F,
) where
    S: Store,
    for<'a> F: Fn(
            &'a ArcRepo,
            &'a S,
            &'a Configuration,
            serde_json::Value,
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    let worker_id = uuid::Uuid::new_v4();

    let yield_limit = 8;
    let mut count = 0;

    loop {
        tracing::trace!("process_jobs: looping");

        count += 1;
        count %= yield_limit;

        // yield every 8 iterations to be kind to other tasks
        if count == 0 {
            tokio::task::yield_now().await;
        }

        let res = job_loop(repo, store, config, worker_id, queue, callback).await;

        if let Err(e) = res {
            tracing::warn!("Error processing jobs: {}", format!("{e}"));
            tracing::warn!("{}", format!("{e:?}"));

            if e.is_disconnected() {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }

            continue;
        }

        break;
    }
}

async fn job_loop<S, F>(
    repo: &ArcRepo,
    store: &S,
    config: &Configuration,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    S: Store,
    for<'a> F: Fn(
            &'a ArcRepo,
            &'a S,
            &'a Configuration,
            serde_json::Value,
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    let yield_limit = 8;
    let mut count = 0;

    loop {
        tracing::trace!("job_loop: looping");

        count += 1;
        count %= yield_limit;

        // yield every 8 iterations to be kind to other tasks
        if count == 0 {
            tokio::task::yield_now().await;
        }

        let fut = async {
            let (job_id, job) = repo.pop(queue, worker_id).await?;

            let span = tracing::info_span!("Running Job");

            let guard = MetricsGuard::guard(worker_id, queue);

            let res = span
                .in_scope(|| {
                    heartbeat(
                        repo,
                        queue,
                        worker_id,
                        job_id,
                        (callback)(repo, store, config, job),
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

#[allow(clippy::too_many_arguments)]
async fn process_image_jobs<S, F>(
    tmp_dir: &ArcTmpDir,
    repo: &ArcRepo,
    store: &S,
    client: &ClientWithMiddleware,
    process_map: &ProcessMap,
    config: &Configuration,
    queue: &'static str,
    callback: F,
) where
    S: Store,
    for<'a> F: Fn(
            &'a ArcTmpDir,
            &'a ArcRepo,
            &'a S,
            &'a ClientWithMiddleware,
            &'a ProcessMap,
            &'a Configuration,
            serde_json::Value,
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    let worker_id = uuid::Uuid::new_v4();

    let yield_limit = 8;
    let mut count = 0;

    loop {
        tracing::trace!("process_image_jobs: looping");

        count += 1;
        count %= yield_limit;

        // yield every 8 iterations to be kind to other tasks
        if count == 0 {
            tokio::task::yield_now().await;
        }

        let res = image_job_loop(
            tmp_dir,
            repo,
            store,
            client,
            process_map,
            config,
            worker_id,
            queue,
            callback,
        )
        .await;

        if let Err(e) = res {
            tracing::warn!("Error processing jobs: {}", format!("{e}"));
            tracing::warn!("{}", format!("{e:?}"));

            if e.is_disconnected() {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }

            continue;
        }

        break;
    }
}

#[allow(clippy::too_many_arguments)]
async fn image_job_loop<S, F>(
    tmp_dir: &ArcTmpDir,
    repo: &ArcRepo,
    store: &S,
    client: &ClientWithMiddleware,
    process_map: &ProcessMap,
    config: &Configuration,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    S: Store,
    for<'a> F: Fn(
            &'a ArcTmpDir,
            &'a ArcRepo,
            &'a S,
            &'a ClientWithMiddleware,
            &'a ProcessMap,
            &'a Configuration,
            serde_json::Value,
        ) -> LocalBoxFuture<'a, Result<(), Error>>
        + Copy,
{
    let yield_limit = 8;
    let mut count = 0;

    loop {
        tracing::trace!("image_job_loop: looping");

        count += 1;
        count %= yield_limit;

        // yield every 8 iterations to be kind to other tasks
        if count == 0 {
            tokio::task::yield_now().await;
        }

        let fut = async {
            let (job_id, job) = repo.pop(queue, worker_id).await?;

            let span = tracing::info_span!("Running Job");

            let guard = MetricsGuard::guard(worker_id, queue);

            let res = span
                .in_scope(|| {
                    heartbeat(
                        repo,
                        queue,
                        worker_id,
                        job_id,
                        (callback)(tmp_dir, repo, store, client, process_map, config, job),
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
    repo: &ArcRepo,
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

    let mut interval = tokio::time::interval(Duration::from_secs(5));

    let mut hb = None;

    let yield_limit = 8;
    let mut count = 0;

    loop {
        tracing::trace!("heartbeat: looping");
        count += 1;
        count %= yield_limit;

        // yield every 8 iterations to be kind to other tasks
        if count == 0 {
            tokio::task::yield_now().await;
        }

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
