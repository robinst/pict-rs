use crate::{
    error::{Error, UploadError},
    queue::{Cleanup, LocalBoxFuture},
    repo::{Alias, AliasRepo, DeleteToken, FullRepo, HashRepo, IdentifierRepo},
    serde_str::Serde,
    store::{Identifier, Store},
};
use tracing::error;

pub(super) fn perform<'a, R, S>(
    repo: &'a R,
    store: &'a S,
    job: &'a [u8],
) -> LocalBoxFuture<'a, Result<(), Error>>
where
    R: FullRepo,
    S: Store,
{
    Box::pin(async move {
        match serde_json::from_slice(job) {
            Ok(job) => match job {
                Cleanup::Hash { hash: in_hash } => hash::<R, S>(repo, in_hash).await?,
                Cleanup::Identifier {
                    identifier: in_identifier,
                } => identifier(repo, &store, in_identifier).await?,
                Cleanup::Alias {
                    alias: stored_alias,
                    token,
                } => {
                    alias(
                        repo,
                        Serde::into_inner(stored_alias),
                        Serde::into_inner(token),
                    )
                    .await?
                }
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
    R: FullRepo,
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
    R: FullRepo,
    S: Store,
{
    let hash: R::Bytes = hash.into();

    let aliases = repo.aliases(hash.clone()).await?;

    if !aliases.is_empty() {
        for alias in aliases {
            let token = repo.delete_token(&alias).await?;
            crate::queue::cleanup_alias(repo, alias, token).await?;
        }
        // Return after queueing cleanup alias, since we will be requeued when the last alias is cleaned
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
        let _ = crate::queue::cleanup_identifier(repo, identifier).await;
    }

    HashRepo::cleanup(repo, hash).await?;

    Ok(())
}

async fn alias<R>(repo: &R, alias: Alias, token: DeleteToken) -> Result<(), Error>
where
    R: FullRepo,
{
    let saved_delete_token = repo.delete_token(&alias).await?;
    if saved_delete_token != token {
        return Err(UploadError::InvalidToken.into());
    }

    let hash = repo.hash(&alias).await?;

    AliasRepo::cleanup(repo, &alias).await?;
    repo.remove_alias(hash.clone(), &alias).await?;

    if repo.aliases(hash.clone()).await?.is_empty() {
        crate::queue::cleanup_hash(repo, hash).await?;
    }

    Ok(())
}
