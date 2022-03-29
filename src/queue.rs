use crate::{
    error::Error,
    repo::{AliasRepo, HashRepo, IdentifierRepo, QueueRepo, Repo},
    store::Store,
};
use tracing::{debug, error, Span};

#[derive(Debug, serde::Deserialize, serde::Serialize)]
enum Job {
    Cleanup { hash: Vec<u8> },
}

pub(crate) async fn queue_cleanup<R: QueueRepo>(repo: &R, hash: R::Bytes) -> Result<(), Error> {
    let job = serde_json::to_vec(&Job::Cleanup {
        hash: hash.as_ref().to_vec(),
    })?;
    repo.push(job.into()).await?;
    Ok(())
}

pub(crate) async fn process_jobs<S: Store>(repo: Repo, store: S, worker_id: Vec<u8>) {
    loop {
        let res = match repo {
            Repo::Sled(ref repo) => do_process_jobs(repo, &store, worker_id.clone()).await,
        };

        if let Err(e) = res {
            tracing::warn!("Error processing jobs: {}", e);
            tracing::warn!("{:?}", e);
            continue;
        }

        break;
    }
}

async fn do_process_jobs<R, S>(repo: &R, store: &S, worker_id: Vec<u8>) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
{
    loop {
        let bytes = repo.pop(worker_id.clone()).await?;

        match serde_json::from_slice(bytes.as_ref()) {
            Ok(job) => match job {
                Job::Cleanup { hash } => cleanup(repo, store, hash).await?,
            },
            Err(e) => {
                tracing::warn!("Invalid job: {}", e);
            }
        }
    }
}

#[tracing::instrument(skip(repo, store))]
async fn cleanup<R, S>(repo: &R, store: &S, hash: Vec<u8>) -> Result<(), Error>
where
    R: HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
{
    let hash: R::Bytes = hash.into();

    let aliases = repo.aliases(hash.clone()).await?;

    if !aliases.is_empty() {
        return Ok(());
    }

    let variant_idents = repo
        .variants::<S::Identifier>(hash.clone())
        .await?
        .into_iter()
        .map(|(_, v)| v)
        .collect::<Vec<_>>();
    let main_ident = repo.identifier(hash.clone()).await?;
    let motion_ident = repo.motion_identifier(hash.clone()).await?;

    HashRepo::cleanup(repo, hash).await?;

    let cleanup_span = tracing::info_span!(parent: None, "Cleaning files");
    cleanup_span.follows_from(Span::current());

    let mut errors = Vec::new();

    for identifier in variant_idents
        .iter()
        .chain(&[main_ident])
        .chain(motion_ident.iter())
    {
        debug!("Deleting {:?}", identifier);
        if let Err(e) = store.remove(identifier).await {
            errors.push(e);
        }

        if let Err(e) = IdentifierRepo::cleanup(repo, identifier).await {
            errors.push(e);
        }
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
