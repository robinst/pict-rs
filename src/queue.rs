use crate::{
    concurrent_processor::ProcessMap,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    future::{LocalBoxFuture, WithPollTimer},
    repo::{Alias, ArcRepo, DeleteToken, Hash, JobId, UploadId},
    serde_str::Serde,
    state::State,
    store::Store,
};

use std::{
    ops::Deref,
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

pub(crate) async fn process_cleanup<S: Store + 'static>(state: State<S>) {
    process_jobs(state, CLEANUP_QUEUE, cleanup::perform).await
}

pub(crate) async fn process_images<S: Store + 'static>(state: State<S>, process_map: ProcessMap) {
    process_image_jobs(state, process_map, PROCESS_QUEUE, process::perform).await
}

struct MetricsGuard {
    worker_id: uuid::Uuid,
    queue: &'static str,
    start: Instant,
    armed: bool,
}

impl MetricsGuard {
    fn guard(worker_id: uuid::Uuid, queue: &'static str) -> Self {
        metrics::counter!(crate::init_metrics::JOB_START, "queue" => queue, "worker-id" => worker_id.to_string()).increment(1);

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
        metrics::histogram!(crate::init_metrics::JOB_DURAION, "queue" => self.queue, "worker-id" => self.worker_id.to_string(), "completed" => (!self.armed).to_string()).record(self.start.elapsed().as_secs_f64());
        metrics::counter!(crate::init_metrics::JOB_END, "queue" => self.queue, "worker-id" => self.worker_id.to_string(), "completed" => (!self.armed).to_string()).increment(1);
    }
}

pub(super) enum JobError {
    Abort(Error),
    Retry(Error),
}

impl AsRef<Error> for JobError {
    fn as_ref(&self) -> &Error {
        match self {
            Self::Abort(e) | Self::Retry(e) => e,
        }
    }
}

impl Deref for JobError {
    type Target = Error;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Abort(e) | Self::Retry(e) => e,
        }
    }
}

impl From<JobError> for Error {
    fn from(value: JobError) -> Self {
        match value {
            JobError::Abort(e) | JobError::Retry(e) => e,
        }
    }
}

type JobResult<T = ()> = Result<T, JobError>;

type JobFuture<'a> = LocalBoxFuture<'a, JobResult>;

trait JobContext {
    type Item;

    fn abort(self) -> JobResult<Self::Item>
    where
        Self: Sized;

    fn retry(self) -> JobResult<Self::Item>
    where
        Self: Sized;
}

impl<T, E> JobContext for Result<T, E>
where
    E: Into<Error>,
{
    type Item = T;

    fn abort(self) -> JobResult<Self::Item>
    where
        Self: Sized,
    {
        self.map_err(Into::into).map_err(JobError::Abort)
    }

    fn retry(self) -> JobResult<Self::Item>
    where
        Self: Sized,
    {
        self.map_err(Into::into).map_err(JobError::Retry)
    }
}

fn job_result(result: &JobResult) -> crate::repo::JobResult {
    match result {
        Ok(()) => crate::repo::JobResult::Success,
        Err(JobError::Retry(_)) => crate::repo::JobResult::Failure,
        Err(JobError::Abort(_)) => crate::repo::JobResult::Aborted,
    }
}

async fn process_jobs<S, F>(state: State<S>, queue: &'static str, callback: F)
where
    S: Store,
    for<'a> F: Fn(&'a State<S>, serde_json::Value) -> JobFuture<'a> + Copy,
{
    let worker_id = uuid::Uuid::new_v4();

    loop {
        tracing::trace!("process_jobs: looping");

        crate::sync::cooperate().await;

        let res = job_loop(&state, worker_id, queue, callback)
            .with_poll_timer("job-loop")
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

async fn job_loop<S, F>(
    state: &State<S>,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    S: Store,
    for<'a> F: Fn(&'a State<S>, serde_json::Value) -> JobFuture<'a> + Copy,
{
    loop {
        tracing::trace!("job_loop: looping");

        crate::sync::cooperate().await;

        async {
            let (job_id, job) = state
                .repo
                .pop(queue, worker_id)
                .with_poll_timer("pop-cleanup")
                .await?;

            let guard = MetricsGuard::guard(worker_id, queue);

            let res = heartbeat(
                &state.repo,
                queue,
                worker_id,
                job_id,
                (callback)(state, job),
            )
            .with_poll_timer("cleanup-job-and-heartbeat")
            .await;

            state
                .repo
                .complete_job(queue, worker_id, job_id, job_result(&res))
                .await?;

            res?;

            guard.disarm();

            Ok(()) as Result<(), Error>
        }
        .instrument(tracing::info_span!("tick", %queue, %worker_id))
        .await?;
    }
}

async fn process_image_jobs<S, F>(
    state: State<S>,
    process_map: ProcessMap,
    queue: &'static str,
    callback: F,
) where
    S: Store,
    for<'a> F: Fn(&'a State<S>, &'a ProcessMap, serde_json::Value) -> JobFuture<'a> + Copy,
{
    let worker_id = uuid::Uuid::new_v4();

    loop {
        tracing::trace!("process_image_jobs: looping");

        crate::sync::cooperate().await;

        let res = image_job_loop(&state, &process_map, worker_id, queue, callback)
            .with_poll_timer("image-job-loop")
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

async fn image_job_loop<S, F>(
    state: &State<S>,
    process_map: &ProcessMap,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    S: Store,
    for<'a> F: Fn(&'a State<S>, &'a ProcessMap, serde_json::Value) -> JobFuture<'a> + Copy,
{
    loop {
        tracing::trace!("image_job_loop: looping");

        crate::sync::cooperate().await;

        async {
            let (job_id, job) = state
                .repo
                .pop(queue, worker_id)
                .with_poll_timer("pop-process")
                .await?;

            let guard = MetricsGuard::guard(worker_id, queue);

            let res = heartbeat(
                &state.repo,
                queue,
                worker_id,
                job_id,
                (callback)(state, process_map, job),
            )
            .with_poll_timer("process-job-and-heartbeat")
            .await;

            state
                .repo
                .complete_job(queue, worker_id, job_id, job_result(&res))
                .await?;

            res?;

            guard.disarm();

            Ok(()) as Result<(), Error>
        }
        .instrument(tracing::info_span!("tick", %queue, %worker_id))
        .await?;
    }
}

#[tracing::instrument("running-job", skip(repo, queue, worker_id, fut))]
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
    let mut fut = std::pin::pin!(fut
        .with_poll_timer("job-future")
        .instrument(tracing::info_span!("job-future")));

    let mut interval = tokio::time::interval(Duration::from_secs(5));

    let mut hb = None;

    loop {
        tracing::trace!("heartbeat: looping");

        crate::sync::cooperate().await;

        tokio::select! {
            biased;
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
