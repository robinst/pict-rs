use crate::{
    error::Error,
    repo::{AliasRepo, HashRepo, IdentifierRepo, QueueRepo, Repo},
    store::Store,
};

mod cleanup;

const CLEANUP_QUEUE: &str = "cleanup";

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Job {
    CleanupHash { hash: Vec<u8> },
    CleanupIdentifier { identifier: Vec<u8> },
}

pub(crate) async fn queue_cleanup<R: QueueRepo>(repo: &R, hash: R::Bytes) -> Result<(), Error> {
    let job = serde_json::to_vec(&Job::CleanupHash {
        hash: hash.as_ref().to_vec(),
    })?;
    repo.push(CLEANUP_QUEUE, job.into()).await?;
    Ok(())
}

pub(crate) async fn process_jobs<S: Store>(repo: Repo, store: S, worker_id: String) {
    match repo {
        Repo::Sled(ref repo) => {
            if let Ok(Some(job)) = repo.in_progress(worker_id.as_bytes().to_vec()).await {
                if let Err(e) = run_job(repo, &store, &job).await {
                    tracing::warn!("Failed to run previously dropped job: {}", e);
                    tracing::warn!("{:?}", e);
                }
            }
            loop {
                let res = job_loop(repo, &store, worker_id.clone()).await;

                if let Err(e) = res {
                    tracing::warn!("Error processing jobs: {}", e);
                    tracing::warn!("{:?}", e);
                    continue;
                }

                break;
            }
        }
    }
}

async fn job_loop<R, S>(repo: &R, store: &S, worker_id: String) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
{
    loop {
        let bytes = repo
            .pop(CLEANUP_QUEUE, worker_id.as_bytes().to_vec())
            .await?;

        run_job(repo, store, bytes.as_ref()).await?;
    }
}

async fn run_job<R, S>(repo: &R, store: &S, job: &[u8]) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
{
    match serde_json::from_slice(job) {
        Ok(job) => match job {
            Job::CleanupHash { hash } => cleanup::hash::<R, S>(repo, hash).await?,
            Job::CleanupIdentifier { identifier } => {
                cleanup::identifier(repo, store, identifier).await?
            }
        },
        Err(e) => {
            tracing::warn!("Invalid job: {}", e);
        }
    }

    Ok(())
}
