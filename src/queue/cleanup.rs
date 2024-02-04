use std::sync::Arc;

use streem::IntoStreamer;
use tracing::{Instrument, Span};

use crate::{
    config::Configuration,
    error::{Error, UploadError},
    future::LocalBoxFuture,
    queue::Cleanup,
    repo::{Alias, ArcRepo, DeleteToken, Hash},
    serde_str::Serde,
    state::State,
    store::Store,
};

pub(super) fn perform<S>(
    state: &State<S>,
    job: serde_json::Value,
) -> LocalBoxFuture<'_, Result<(), Error>>
where
    S: Store + 'static,
{
    Box::pin(async move {
        match serde_json::from_value(job) {
            Ok(job) => match job {
                Cleanup::Hash { hash: in_hash } => hash(&state.repo, in_hash).await?,
                Cleanup::Identifier {
                    identifier: in_identifier,
                } => identifier(&state.repo, &state.store, Arc::from(in_identifier)).await?,
                Cleanup::Alias {
                    alias: stored_alias,
                    token,
                } => {
                    alias(
                        &state.repo,
                        Serde::into_inner(stored_alias),
                        Serde::into_inner(token),
                    )
                    .await?
                }
                Cleanup::Variant { hash, variant } => {
                    hash_variant(&state.repo, hash, variant).await?
                }
                Cleanup::AllVariants => all_variants(&state.repo).await?,
                Cleanup::OutdatedVariants => outdated_variants(&state.repo, &state.config).await?,
                Cleanup::OutdatedProxies => outdated_proxies(&state.repo, &state.config).await?,
                Cleanup::Prune => prune(&state.repo, &state.store).await?,
            },
            Err(e) => {
                tracing::warn!("Invalid job: {}", format!("{e}"));
            }
        }

        Ok(())
    })
}

#[tracing::instrument(skip_all)]
async fn identifier<S>(repo: &ArcRepo, store: &S, identifier: Arc<str>) -> Result<(), Error>
where
    S: Store,
{
    let mut errors = Vec::new();

    if let Err(e) = store.remove(&identifier).await {
        errors.push(UploadError::from(e));
    }

    if let Err(e) = repo.cleanup_details(&identifier).await {
        errors.push(UploadError::from(e));
    }

    for error in errors {
        tracing::error!("{}", format!("{error:?}"));
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn hash(repo: &ArcRepo, hash: Hash) -> Result<(), Error> {
    let aliases = repo.aliases_for_hash(hash.clone()).await?;

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
        let _ = super::cleanup_identifier(repo, &identifier).await;
    }

    repo.cleanup_hash(hash).await?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(crate) async fn alias(repo: &ArcRepo, alias: Alias, token: DeleteToken) -> Result<(), Error> {
    let saved_delete_token = repo.delete_token(&alias).await?;

    if !saved_delete_token.is_some_and(|t| t.ct_eq(&token)) {
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

    if repo.aliases_for_hash(hash.clone()).await?.is_empty() {
        super::cleanup_hash(repo, hash).await?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn all_variants(repo: &ArcRepo) -> Result<(), Error> {
    let hash_stream = std::pin::pin!(repo.hashes());
    let mut hash_stream = hash_stream.into_streamer();

    while let Some(res) = hash_stream.next().await {
        tracing::trace!("all_variants: looping");

        let hash = res?;
        super::cleanup_variants(repo, hash, None).await?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn outdated_variants(repo: &ArcRepo, config: &Configuration) -> Result<(), Error> {
    let now = time::OffsetDateTime::now_utc();
    let since = now.saturating_sub(config.media.retention.variants.to_duration());

    let variant_stream = repo.older_variants(since).await?;
    let variant_stream = std::pin::pin!(crate::stream::take(variant_stream, 2048));
    let mut variant_stream = variant_stream.into_streamer();

    let mut count = 0;

    while let Some(res) = variant_stream.next().await {
        metrics::counter!(crate::init_metrics::CLEANUP_OUTDATED_VARIANT).increment(1);
        tracing::trace!("outdated_variants: looping");

        let (hash, variant) = res?;
        super::cleanup_variants(repo, hash, Some(variant)).await?;
        count += 1;
    }

    tracing::debug!("Queued {count} variant cleanup jobs");
    let queue_length = repo.queue_length().await?;
    tracing::debug!("Total queue length: {queue_length}");

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn outdated_proxies(repo: &ArcRepo, config: &Configuration) -> Result<(), Error> {
    let now = time::OffsetDateTime::now_utc();
    let since = now.saturating_sub(config.media.retention.proxy.to_duration());

    let alias_stream = repo.older_aliases(since).await?;
    let alias_stream = std::pin::pin!(crate::stream::take(alias_stream, 2048));
    let mut alias_stream = alias_stream.into_streamer();

    let mut count = 0;

    while let Some(res) = alias_stream.next().await {
        metrics::counter!(crate::init_metrics::CLEANUP_OUTDATED_PROXY).increment(1);
        tracing::trace!("outdated_proxies: looping");

        let alias = res?;
        if let Some(token) = repo.delete_token(&alias).await? {
            super::cleanup_alias(repo, alias, token).await?;
            count += 1;
        } else {
            tracing::warn!("Skipping alias cleanup - no delete token");
            repo.remove_relation(alias.clone()).await?;
            repo.remove_alias_access(alias).await?;
        }
    }

    tracing::debug!("Queued {count} alias cleanup jobs");
    let queue_length = repo.queue_length().await?;
    tracing::debug!("Total queue length: {queue_length}");

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
            super::cleanup_identifier(repo, &identifier).await?;
        }

        repo.remove_variant(hash.clone(), target_variant.clone())
            .await?;
        repo.remove_variant_access(hash, target_variant).await?;
    } else {
        for (variant, identifier) in repo.variants(hash.clone()).await? {
            repo.remove_variant(hash.clone(), variant.clone()).await?;
            repo.remove_variant_access(hash.clone(), variant).await?;
            super::cleanup_identifier(repo, &identifier).await?;
        }
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn prune<S>(repo: &ArcRepo, store: &S) -> Result<(), Error>
where
    S: Store + 'static,
{
    repo.set("prune-missing-started", b"1".to_vec().into())
        .await?;

    let hash_stream = std::pin::pin!(repo.hashes());
    let mut hash_stream = hash_stream.into_streamer();

    let mut count: u64 = 0;

    while let Some(hash) = hash_stream.try_next().await? {
        tracing::trace!("prune: looping");

        let repo = repo.clone();
        let store = store.clone();

        let current_span = Span::current();

        let span = tracing::info_span!(parent: current_span, "error-boundary");

        let res = crate::sync::abort_on_drop(crate::sync::spawn(
            "prune-missing",
            async move {
                let mut count = count;

                if let Some(ident) = repo.identifier(hash.clone()).await? {
                    match store.len(&ident).await {
                        Err(e) if e.is_not_found() => {
                            super::cleanup_hash(&repo, hash).await?;

                            count += 1;

                            repo.set(
                                "prune-missing-queued",
                                Vec::from(count.to_be_bytes()).into(),
                            )
                            .await?;
                        }
                        _ => (),
                    }
                }

                Ok(count) as Result<u64, Error>
            }
            .instrument(span),
        ))
        .await;

        match res {
            Ok(Ok(updated)) => count = updated,
            Ok(Err(e)) => {
                tracing::warn!("Prune missing identifier failed - {e:?}");
            }
            Err(_) => {
                tracing::warn!("Prune missing identifier panicked.");
            }
        }

        count += 1;
    }

    repo.set("prune-missing-complete", b"1".to_vec().into())
        .await?;

    Ok(())
}
