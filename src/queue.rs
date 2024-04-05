use crate::{
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    future::{LocalBoxFuture, WithPollTimer},
    repo::{Alias, ArcRepo, DeleteToken, Hash, JobId, UploadId},
    serde_str::Serde,
    state::State,
    store::Store,
    UploadQuery,
};

use std::{
    ops::Deref,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinError;
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
        #[serde(default)]
        upload_query: UploadQuery,
    },
    Generate {
        target_format: InputProcessableFormat,
        source: Serde<Alias>,
        process_path: String,
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
    upload_query: UploadQuery,
) -> Result<(), Error> {
    let job = serde_json::to_value(Process::Ingest {
        identifier: identifier.to_string(),
        declared_alias: declared_alias.map(Serde::new),
        upload_id: Serde::new(upload_id),
        upload_query,
    })
    .map_err(UploadError::PushJob)?;
    repo.push(PROCESS_QUEUE, job, None).await?;
    Ok(())
}

pub(crate) async fn queue_generate(
    repo: &ArcRepo,
    target_format: InputProcessableFormat,
    source: Alias,
    variant: String,
    process_args: Vec<String>,
) -> Result<(), Error> {
    let job = serde_json::to_value(Process::Generate {
        target_format,
        source: Serde::new(source),
        process_path: variant,
        process_args,
    })
    .map_err(UploadError::PushJob)?;
    repo.push(PROCESS_QUEUE, job, None).await?;
    Ok(())
}

pub(crate) async fn process_cleanup<S: Store + 'static>(state: State<S>) {
    process_jobs(state, CLEANUP_QUEUE, cleanup::perform).await
}

pub(crate) async fn process_images<S: Store + 'static>(state: State<S>) {
    process_jobs(state, PROCESS_QUEUE, process::perform).await
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

fn job_result(result: &Result<JobResult, JoinError>) -> crate::repo::JobResult {
    match result {
        Ok(Ok(())) => crate::repo::JobResult::Success,
        Ok(Err(JobError::Retry(_))) => crate::repo::JobResult::Failure,
        Ok(Err(JobError::Abort(_))) => crate::repo::JobResult::Aborted,
        Err(_) => crate::repo::JobResult::Aborted,
    }
}

async fn process_jobs<S, F>(state: State<S>, queue: &'static str, callback: F)
where
    S: Store + 'static,
    for<'a> F: Fn(&'a State<S>, serde_json::Value) -> JobFuture<'a> + Copy + 'static,
{
    let worker_id = uuid::Uuid::new_v4();
    let state = Rc::new(state);

    loop {
        tracing::trace!("process_jobs: looping");

        crate::sync::cooperate().await;

        // add a panic boundary by spawning a task
        let res = crate::sync::spawn(
            "job-loop",
            job_loop(state.clone(), worker_id, queue, callback),
        )
        .await;

        match res {
            // clean exit
            Ok(Ok(())) => break,

            // job error
            Ok(Err(e)) => {
                tracing::warn!("Error processing jobs: {}", format!("{e}"));
                tracing::warn!("{}", format!("{e:?}"));

                if e.is_disconnected() {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }

            // job panic
            Err(_) => {
                tracing::warn!("Panic while processing jobs");
            }
        }
    }
}

async fn job_loop<S, F>(
    state: Rc<State<S>>,
    worker_id: uuid::Uuid,
    queue: &'static str,
    callback: F,
) -> Result<(), Error>
where
    S: Store + 'static,
    for<'a> F: Fn(&'a State<S>, serde_json::Value) -> JobFuture<'a> + Copy + 'static,
{
    loop {
        tracing::trace!("job_loop: looping");

        crate::sync::cooperate().with_poll_timer("cooperate").await;

        async {
            let (job_id, job) = state
                .repo
                .pop(queue, worker_id)
                .with_poll_timer("pop-job")
                .await?;

            let guard = MetricsGuard::guard(worker_id, queue);

            let state2 = state.clone();
            let res = crate::sync::spawn("job-and-heartbeat", async move {
                let state = state2;
                heartbeat(
                    &state.repo,
                    queue,
                    worker_id,
                    job_id,
                    (callback)(&state, job),
                )
                .await
            })
            .await;

            state
                .repo
                .complete_job(queue, worker_id, job_id, job_result(&res))
                .with_poll_timer("job-complete")
                .await?;

            res.map_err(|_| UploadError::Canceled)??;

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
