use crate::{
    config::Configuration,
    error::{Error, UploadError},
    queue::{Base64Bytes, Cleanup, LocalBoxFuture},
    repo::{Alias, ArcRepo, DeleteToken, Hash},
    serde_str::Serde,
    store::{Identifier, Store},
};
use futures_util::StreamExt;

pub(super) fn perform<'a, S>(
    repo: &'a ArcRepo,
    store: &'a S,
    configuration: &'a Configuration,
    job: &'a [u8],
) -> LocalBoxFuture<'a, Result<(), Error>>
where
    S: Store,
{
    Box::pin(async move {
        match serde_json::from_slice(job) {
            Ok(job) => match job {
                Cleanup::Hash { hash: in_hash } => hash(repo, in_hash).await?,
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
                Cleanup::Variant { hash, variant } => hash_variant(repo, hash, variant).await?,
                Cleanup::AllVariants => all_variants(repo).await?,
                Cleanup::OutdatedVariants => outdated_variants(repo, configuration).await?,
                Cleanup::OutdatedProxies => outdated_proxies(repo, configuration).await?,
            },
            Err(e) => {
                tracing::warn!("Invalid job: {}", format!("{e}"));
            }
        }

        Ok(())
    })
}

#[tracing::instrument(skip_all)]
async fn identifier<S>(repo: &ArcRepo, store: &S, identifier: Vec<u8>) -> Result<(), Error>
where
    S: Store,
{
    let identifier = S::Identifier::from_bytes(identifier)?;

    let mut errors = Vec::new();

    if let Err(e) = store.remove(&identifier).await {
        errors.push(e);
    }

    if let Err(e) = repo.cleanup_details(&identifier).await {
        errors.push(e);
    }

    for error in errors {
        tracing::error!("{}", format!("{error:?}"));
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn hash(repo: &ArcRepo, hash: Hash) -> Result<(), Error> {
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
        .variants(hash.clone())
        .await?
        .into_iter()
        .map(|(_, v)| v)
        .collect::<Vec<_>>();
    idents.extend(repo.identifier(hash.clone()).await?);
    idents.extend(repo.motion_identifier(hash.clone()).await?);

    for identifier in idents {
        let _ = super::cleanup_identifier(repo, identifier).await;
    }

    repo.cleanup_hash(hash).await?;

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn alias(repo: &ArcRepo, alias: Alias, token: DeleteToken) -> Result<(), Error> {
    let saved_delete_token = repo.delete_token(&alias).await?;

    if saved_delete_token.is_some() && saved_delete_token != Some(token) {
        return Err(UploadError::InvalidToken.into());
    }

    let hash = repo.hash(&alias).await?;

    repo.cleanup_alias(&alias).await?;
    repo.remove_relation(alias.clone()).await?;
    repo.remove_alias_access(alias.clone()).await?;

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
async fn all_variants(repo: &ArcRepo) -> Result<(), Error> {
    let mut hash_stream = Box::pin(repo.hashes().await);

    while let Some(res) = hash_stream.next().await {
        let hash = res?;
        super::cleanup_variants(repo, hash, None).await?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn outdated_variants(repo: &ArcRepo, config: &Configuration) -> Result<(), Error> {
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
async fn outdated_proxies(repo: &ArcRepo, config: &Configuration) -> Result<(), Error> {
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
            repo.remove_alias_access(alias).await?;
        }
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn hash_variant(
    repo: &ArcRepo,
    hash: Hash,
    target_variant: Option<String>,
) -> Result<(), Error> {
    if let Some(target_variant) = target_variant {
        if let Some(identifier) = repo
            .variant_identifier(hash.clone(), target_variant.clone())
            .await?
        {
            super::cleanup_identifier(repo, identifier).await?;
        }

        repo.remove_variant(hash.clone(), target_variant.clone())
            .await?;
        repo.remove_variant_access(hash, target_variant).await?;
    } else {
        for (variant, identifier) in repo.variants(hash.clone()).await? {
            repo.remove_variant(hash.clone(), variant.clone()).await?;
            repo.remove_variant_access(hash.clone(), variant).await?;
            super::cleanup_identifier(repo, identifier).await?;
        }
    }

    Ok(())
}
