use crate::{
    error::Error,
    repo::{AliasRepo, HashRepo, IdentifierRepo, QueueRepo, Repo},
    store::{Identifier, Store},
};
use tracing::{debug, error};

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Job {
    CleanupHash { hash: Vec<u8> },
    CleanupIdentifier { identifier: Vec<u8> },
}

pub(crate) async fn queue_cleanup<R: QueueRepo>(repo: &R, hash: R::Bytes) -> Result<(), Error> {
    let job = serde_json::to_vec(&Job::CleanupHash {
        hash: hash.as_ref().to_vec(),
    })?;
    repo.push(job.into()).await?;
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
        let bytes = repo.pop(worker_id.as_bytes().to_vec()).await?;

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
            Job::CleanupHash { hash } => cleanup_hash::<R, S>(repo, hash).await?,
            Job::CleanupIdentifier { identifier } => {
                cleanup_identifier(repo, store, identifier).await?
            }
        },
        Err(e) => {
            tracing::warn!("Invalid job: {}", e);
        }
    }

    Ok(())
}

#[tracing::instrument(skip(repo, store))]
async fn cleanup_identifier<R, S>(repo: &R, store: &S, identifier: Vec<u8>) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo,
    R::Bytes: Clone,
    S: Store,
{
    let identifier = S::Identifier::from_bytes(identifier)?;

    let mut errors = Vec::new();

    debug!("Deleting {:?}", identifier);
    if let Err(e) = store.remove(&identifier).await {
        errors.push(e);
    }

    if let Err(e) = IdentifierRepo::cleanup(repo, &identifier).await {
        errors.push(e);
    }

    if !errors.is_empty() {
        let span = tracing::error_span!("Error deleting files");
        span.in_scope(|| {
            for error in errors {
                error!("{}", error);
            }
        });
    }

    Ok(())
}

#[tracing::instrument(skip(repo))]
async fn cleanup_hash<R, S>(repo: &R, hash: Vec<u8>) -> Result<(), Error>
where
    R: QueueRepo + AliasRepo + HashRepo + IdentifierRepo,
    R::Bytes: Clone,
    S: Store,
{
    let hash: R::Bytes = hash.into();

    let aliases = repo.aliases(hash.clone()).await?;

    if !aliases.is_empty() {
        return Ok(());
    }

    let mut idents = repo
        .variants::<S::Identifier>(hash.clone())
        .await?
        .into_iter()
        .map(|(_, v)| v)
        .collect::<Vec<_>>();
    idents.push(repo.identifier(hash.clone()).await?);
    idents.extend(repo.motion_identifier(hash.clone()).await?);

    for identifier in idents {
        if let Ok(identifier) = identifier.to_bytes() {
            let job = serde_json::to_vec(&Job::CleanupIdentifier { identifier })?;
            repo.push(job.into()).await?;
        }
    }

    HashRepo::cleanup(repo, hash).await?;

    Ok(())
}
