use crate::{
    error::{Error, UploadError},
    queue::{Base64Bytes, Cleanup, LocalBoxFuture},
    repo::{Alias, AliasRepo, DeleteToken, FullRepo, HashRepo, IdentifierRepo},
    serde_str::Serde,
    store::{Identifier, Store},
};
use futures_util::StreamExt;

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
                Cleanup::Hash {
                    hash: Base64Bytes(in_hash),
                } => hash::<R, S>(repo, in_hash).await?,
                Cleanup::Identifier {
                    identifier: Base64Bytes(in_identifier),
                } => identifier(repo, store, in_identifier).await?,
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
                Cleanup::Variant {
                    hash: Base64Bytes(hash),
                } => variant::<R, S>(repo, hash).await?,
                Cleanup::AllVariants => all_variants::<R, S>(repo).await?,
            },
            Err(e) => {
                tracing::warn!("Invalid job: {}", format!("{e}"));
            }
        }

        Ok(())
    })
}

#[tracing::instrument(skip_all)]
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
                tracing::error!("{}", format!("{error}"));
            }
        });
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn hash<R, S>(repo: &R, hash: Vec<u8>) -> Result<(), Error>
where
    R: FullRepo,
    S: Store,
{
    let hash: R::Bytes = hash.into();

    let aliases = repo.aliases(hash.clone()).await?;

    if !aliases.is_empty() {
        for alias in aliases {
            // TODO: decide if it is okay to skip aliases without tokens
            if let Some(token) = repo.delete_token(&alias).await? {
                super::cleanup_alias(repo, alias, token).await?;
            }
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
    idents.extend(repo.identifier(hash.clone()).await?);
    idents.extend(repo.motion_identifier(hash.clone()).await?);

    for identifier in idents {
        let _ = super::cleanup_identifier(repo, identifier).await;
    }

    HashRepo::cleanup(repo, hash).await?;

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn alias<R>(repo: &R, alias: Alias, token: DeleteToken) -> Result<(), Error>
where
    R: FullRepo,
{
    let saved_delete_token = repo.delete_token(&alias).await?;

    if saved_delete_token.is_some() && saved_delete_token != Some(token) {
        return Err(UploadError::InvalidToken.into());
    }

    AliasRepo::cleanup(repo, &alias).await?;

    let Some(hash) = repo.hash(&alias).await? else {
        // hash doesn't exist, nothing to do
        return Ok(());
    };

    repo.remove_alias(hash.clone(), &alias).await?;

    if repo.aliases(hash.clone()).await?.is_empty() {
        super::cleanup_hash(repo, hash).await?;
    }

    Ok(())
}

async fn all_variants<R, S>(repo: &R) -> Result<(), Error>
where
    R: FullRepo,
    S: Store,
{
    let mut hash_stream = Box::pin(repo.hashes().await);

    while let Some(res) = hash_stream.next().await {
        let hash = res?;
        super::cleanup_variants(repo, hash).await?;
    }

    Ok(())
}

async fn variant<R, S>(repo: &R, hash: Vec<u8>) -> Result<(), Error>
where
    R: FullRepo,
    S: Store,
{
    let hash: R::Bytes = hash.into();

    for (variant, identifier) in repo.variants::<S::Identifier>(hash.clone()).await? {
        repo.remove_variant(hash.clone(), variant).await?;
        super::cleanup_identifier(repo, identifier).await?;
    }

    Ok(())
}
