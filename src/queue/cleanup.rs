use crate::{
    config::Configuration,
    error::{Error, UploadError},
    queue::{Base64Bytes, Cleanup, LocalBoxFuture},
    repo::{
        Alias, AliasAccessRepo, AliasRepo, DeleteToken, FullRepo, HashRepo, IdentifierRepo,
        VariantAccessRepo,
    },
    serde_str::Serde,
    store::{Identifier, Store},
};
use futures_util::StreamExt;

pub(super) fn perform<'a, R, S>(
    repo: &'a R,
    store: &'a S,
    configuration: &'a Configuration,
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
                    variant,
                } => hash_variant::<R, S>(repo, hash, variant).await?,
                Cleanup::AllVariants => all_variants::<R, S>(repo).await?,
                Cleanup::OutdatedVariants => outdated_variants::<R, S>(repo, configuration).await?,
                Cleanup::OutdatedProxies => outdated_proxies::<R, S>(repo, configuration).await?,
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

    let aliases = repo.for_hash(hash.clone()).await?;

    if !aliases.is_empty() {
        for alias in aliases {
            // TODO: decide if it is okay to skip aliases without tokens
            if let Some(token) = repo.delete_token(&alias).await? {
                super::cleanup_alias(repo, alias, token).await?;
            } else {
                tracing::warn!("Not cleaning alias!");
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

    let hash = repo.hash(&alias).await?;

    AliasRepo::cleanup(repo, &alias).await?;
    repo.remove_relation(alias.clone()).await?;
    AliasAccessRepo::remove_access(repo, alias.clone()).await?;

    let Some(hash) = hash else {
        // hash doesn't exist, nothing to do
        return Ok(());
    };

    if repo.for_hash(hash.clone()).await?.is_empty() {
        super::cleanup_hash(repo, hash).await?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn all_variants<R, S>(repo: &R) -> Result<(), Error>
where
    R: FullRepo,
    S: Store,
{
    let mut hash_stream = Box::pin(repo.hashes().await);

    while let Some(res) = hash_stream.next().await {
        let hash = res?;
        super::cleanup_variants(repo, hash, None).await?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn outdated_variants<R, S>(repo: &R, config: &Configuration) -> Result<(), Error>
where
    R: FullRepo,
    S: Store,
{
    let now = time::OffsetDateTime::now_utc();
    let since = now.saturating_sub(config.media.retention.variants.to_duration());

    let mut variant_stream = Box::pin(repo.older_variants(since).await?);

    while let Some(res) = variant_stream.next().await {
        let (hash, variant) = res?;
        super::cleanup_variants(repo, hash, Some(variant)).await?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn outdated_proxies<R, S>(repo: &R, config: &Configuration) -> Result<(), Error>
where
    R: FullRepo,
    S: Store,
{
    let now = time::OffsetDateTime::now_utc();
    let since = now.saturating_sub(config.media.retention.proxy.to_duration());

    let mut alias_stream = Box::pin(repo.older_aliases(since).await?);

    while let Some(res) = alias_stream.next().await {
        let alias = res?;
        if let Some(token) = repo.delete_token(&alias).await? {
            super::cleanup_alias(repo, alias, token).await?;
        } else {
            tracing::warn!("Skipping alias cleanup - no delete token");
            repo.remove_relation(alias.clone()).await?;
            AliasAccessRepo::remove_access(repo, alias).await?;
        }
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn hash_variant<R, S>(
    repo: &R,
    hash: Vec<u8>,
    target_variant: Option<String>,
) -> Result<(), Error>
where
    R: FullRepo,
    S: Store,
{
    let hash: R::Bytes = hash.into();

    if let Some(target_variant) = target_variant {
        if let Some(identifier) = repo
            .variant_identifier::<S::Identifier>(hash.clone(), target_variant.clone())
            .await?
        {
            super::cleanup_identifier(repo, identifier).await?;
        }

        repo.remove_variant(hash.clone(), target_variant.clone())
            .await?;
        VariantAccessRepo::remove_access(repo, hash, target_variant).await?;
    } else {
        for (variant, identifier) in repo.variants::<S::Identifier>(hash.clone()).await? {
            repo.remove_variant(hash.clone(), variant.clone()).await?;
            VariantAccessRepo::remove_access(repo, hash.clone(), variant).await?;
            super::cleanup_identifier(repo, identifier).await?;
        }
    }

    Ok(())
}
