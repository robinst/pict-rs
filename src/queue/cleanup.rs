use std::sync::Arc;

use streem::IntoStreamer;
use tracing::{Instrument, Span};

use crate::{
    config::Configuration,
    error::{Error, UploadError},
    future::WithPollTimer,
    queue::Cleanup,
    repo::{Alias, ArcRepo, DeleteToken, Hash},
    serde_str::Serde,
    state::State,
    store::Store,
};

use super::{JobContext, JobFuture, JobResult};

pub(super) fn perform<S>(state: &State<S>, job: serde_json::Value) -> JobFuture<'_>
where
    S: Store + 'static,
{
    Box::pin(async move {
        let job_text = format!("{job}");

        let job = serde_json::from_value(job)
            .map_err(|e| UploadError::InvalidJob(e, job_text))
            .abort()?;

        match job {
            Cleanup::Hash { hash: in_hash } => {
                hash(&state.repo, in_hash)
                    .with_poll_timer("cleanup-hash")
                    .await?
            }
            Cleanup::Identifier {
                identifier: in_identifier,
            } => {
                identifier(&state.repo, &state.store, Arc::from(in_identifier))
                    .with_poll_timer("cleanup-identifier")
                    .await?
            }
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
                hash_variant(&state.repo, hash, variant)
                    .with_poll_timer("cleanup-hash-variant")
                    .await?
            }
            Cleanup::AllVariants => {
                all_variants(&state.repo)
                    .with_poll_timer("cleanup-all-variants")
                    .await?
            }
            Cleanup::OutdatedVariants => {
                outdated_variants(&state.repo, &state.config)
                    .with_poll_timer("cleanup-outdated-variants")
                    .await?
            }
            Cleanup::OutdatedProxies => {
                outdated_proxies(&state.repo, &state.config)
                    .with_poll_timer("cleanup-outdated-proxies")
                    .await?
            }
            Cleanup::Prune => {
                prune(&state.repo, &state.store)
                    .with_poll_timer("cleanup-prune")
                    .await?
            }
        }

        Ok(())
    })
}

#[tracing::instrument(skip_all)]
async fn identifier<S>(repo: &ArcRepo, store: &S, identifier: Arc<str>) -> JobResult
where
    S: Store,
{
    match store.remove(&identifier).await {
        Ok(_) => {}
        Err(e) if e.is_not_found() => {}
        Err(e) => return Err(e).retry(),
    }

    repo.cleanup_details(&identifier).await.retry()?;

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn hash(repo: &ArcRepo, hash: Hash) -> JobResult {
    let aliases = repo.aliases_for_hash(hash.clone()).await.retry()?;

    if !aliases.is_empty() {
        for alias in aliases {
            // TODO: decide if it is okay to skip aliases without tokens
            if let Some(token) = repo.delete_token(&alias).await.retry()? {
                super::cleanup_alias(repo, alias, token).await.retry()?;
            } else {
                tracing::warn!("Not cleaning alias!");
            }
        }
        // Return after queueing cleanup alias, since we will be requeued when the last alias is cleaned
        return Ok(());
    }

    let mut idents = repo
        .variants(hash.clone())
        .await
        .retry()?
        .into_iter()
        .map(|(_, v)| v)
        .collect::<Vec<_>>();
    idents.extend(repo.identifier(hash.clone()).await.retry()?);
    idents.extend(repo.motion_identifier(hash.clone()).await.retry()?);

    for identifier in idents {
        super::cleanup_identifier(repo, &identifier).await.retry()?;
    }

    repo.cleanup_hash(hash).await.retry()?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(crate) async fn alias(repo: &ArcRepo, alias: Alias, token: DeleteToken) -> JobResult {
    let saved_delete_token = repo.delete_token(&alias).await.retry()?;

    if !saved_delete_token.is_some_and(|t| t.ct_eq(&token)) {
        return Err(UploadError::InvalidToken).abort();
    }

    let hash = repo.hash(&alias).await.retry()?;

    repo.cleanup_alias(&alias).await.retry()?;
    repo.remove_relation(alias.clone()).await.retry()?;
    repo.remove_alias_access(alias.clone()).await.retry()?;

    let hash = hash.ok_or(UploadError::MissingAlias).abort()?;

    if repo
        .aliases_for_hash(hash.clone())
        .await
        .retry()?
        .is_empty()
    {
        super::cleanup_hash(repo, hash).await.retry()?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn all_variants(repo: &ArcRepo) -> JobResult {
    let hash_stream = std::pin::pin!(repo.hashes());
    let mut hash_stream = hash_stream.into_streamer();

    while let Some(res) = hash_stream.next().await {
        tracing::trace!("all_variants: looping");

        let hash = res.retry()?;
        super::cleanup_variants(repo, hash, None).await.retry()?;
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn outdated_variants(repo: &ArcRepo, config: &Configuration) -> JobResult {
    let now = time::OffsetDateTime::now_utc();
    let since = now.saturating_sub(config.media.retention.variants.to_duration());

    let variant_stream = repo.older_variants(since).await.retry()?;
    let variant_stream = std::pin::pin!(crate::stream::take(variant_stream, 2048));
    let mut variant_stream = variant_stream.into_streamer();

    let mut count = 0;

    while let Some((hash, variant)) = variant_stream.try_next().await.retry()? {
        metrics::counter!(crate::init_metrics::CLEANUP_OUTDATED_VARIANT).increment(1);
        tracing::trace!("outdated_variants: looping");

        super::cleanup_variants(repo, hash, Some(variant))
            .await
            .retry()?;
        count += 1;
    }

    tracing::debug!("Queued {count} variant cleanup jobs");
    let queue_length = repo.queue_length().await.abort()?;
    tracing::debug!("Total queue length: {queue_length}");

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn outdated_proxies(repo: &ArcRepo, config: &Configuration) -> JobResult {
    let now = time::OffsetDateTime::now_utc();
    let since = now.saturating_sub(config.media.retention.proxy.to_duration());

    let alias_stream = repo.older_aliases(since).await.retry()?;
    let alias_stream = std::pin::pin!(crate::stream::take(alias_stream, 2048));
    let mut alias_stream = alias_stream.into_streamer();

    let mut count = 0;

    while let Some(alias) = alias_stream.try_next().await.retry()? {
        metrics::counter!(crate::init_metrics::CLEANUP_OUTDATED_PROXY).increment(1);
        tracing::trace!("outdated_proxies: looping");

        if let Some(token) = repo.delete_token(&alias).await.retry()? {
            super::cleanup_alias(repo, alias, token).await.retry()?;
            count += 1;
        } else {
            tracing::warn!("Skipping alias cleanup - no delete token");
            repo.remove_relation(alias.clone()).await.retry()?;
            repo.remove_alias_access(alias).await.retry()?;
        }
    }

    tracing::debug!("Queued {count} alias cleanup jobs");
    let queue_length = repo.queue_length().await.abort()?;
    tracing::debug!("Total queue length: {queue_length}");

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn hash_variant(repo: &ArcRepo, hash: Hash, target_variant: Option<String>) -> JobResult {
    if let Some(target_variant) = target_variant {
        if let Some(identifier) = repo
            .variant_identifier(hash.clone(), target_variant.clone())
            .await
            .retry()?
        {
            super::cleanup_identifier(repo, &identifier).await.retry()?;
        }

        repo.remove_variant(hash.clone(), target_variant.clone())
            .await
            .retry()?;
        repo.remove_variant_access(hash, target_variant)
            .await
            .retry()?;
    } else {
        for (variant, identifier) in repo.variants(hash.clone()).await.retry()? {
            repo.remove_variant(hash.clone(), variant.clone())
                .await
                .retry()?;
            repo.remove_variant_access(hash.clone(), variant)
                .await
                .retry()?;
            super::cleanup_identifier(repo, &identifier).await.retry()?;
        }
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn prune<S>(repo: &ArcRepo, store: &S) -> JobResult
where
    S: Store + 'static,
{
    repo.set("prune-missing-started", b"1".to_vec().into())
        .await
        .retry()?;

    let hash_stream = std::pin::pin!(repo.hashes());
    let mut hash_stream = hash_stream.into_streamer();

    let mut count: u64 = 0;

    while let Some(hash) = hash_stream.try_next().await.retry()? {
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
        .await
        .retry()?;

    Ok(())
}
