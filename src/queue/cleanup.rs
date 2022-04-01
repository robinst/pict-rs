use crate::{
    error::Error,
    queue::{Cleanup, LocalBoxFuture, CLEANUP_QUEUE},
    repo::{AliasRepo, HashRepo, IdentifierRepo, QueueRepo},
    store::{Identifier, Store},
};
use tracing::error;

pub(super) fn perform<'a, R, S>(
    repo: &'a R,
    store: &'a S,
    job: &'a [u8],
) -> LocalBoxFuture<'a, Result<(), Error>>
where
    R: QueueRepo + HashRepo + IdentifierRepo + AliasRepo,
    R::Bytes: Clone,
    S: Store,
{
    Box::pin(async move {
        match serde_json::from_slice(job) {
            Ok(job) => match job {
                Cleanup::CleanupHash { hash: in_hash } => hash::<R, S>(repo, in_hash).await?,
                Cleanup::CleanupIdentifier {
                    identifier: in_identifier,
                } => identifier(repo, &store, in_identifier).await?,
            },
            Err(e) => {
                tracing::warn!("Invalid job: {}", e);
            }
        }

        Ok(())
    })
}

#[tracing::instrument(skip(repo, store))]
async fn identifier<R, S>(repo: &R, store: &S, identifier: Vec<u8>) -> Result<(), Error>
where
    R: QueueRepo + HashRepo + IdentifierRepo,
    R::Bytes: Clone,
    S: Store,
{
    let identifier = S::Identifier::from_bytes(identifier)?;

    let mut errors = Vec::new();

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
async fn hash<R, S>(repo: &R, hash: Vec<u8>) -> Result<(), Error>
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
            let job = serde_json::to_vec(&Cleanup::CleanupIdentifier { identifier })?;
            repo.push(CLEANUP_QUEUE, job.into()).await?;
        }
    }

    HashRepo::cleanup(repo, hash).await?;

    Ok(())
}
