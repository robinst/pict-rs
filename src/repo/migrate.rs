use std::sync::{Arc, OnceLock};

use streem::IntoStreamer;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    config::Configuration,
    details::Details,
    error::Error,
    repo::{ArcRepo, DeleteToken, Hash},
    repo_04::{
        AliasRepo as _, HashRepo as _, IdentifierRepo as _, SettingsRepo as _,
        SledRepo as OldSledRepo,
    },
    store::Store,
    tmp_file::{ArcTmpDir, TmpDir},
};

const GENERATOR_KEY: &str = "last-path";

#[tracing::instrument(skip_all)]
pub(crate) async fn migrate_repo(old_repo: ArcRepo, new_repo: ArcRepo) -> Result<(), Error> {
    tracing::info!("Running checks");
    if let Err(e) = old_repo.health_check().await {
        tracing::warn!("Old repo is not configured correctly");
        return Err(e.into());
    }
    if let Err(e) = new_repo.health_check().await {
        tracing::warn!("New repo is not configured correctly");
        return Err(e.into());
    }

    let total_size = old_repo.size().await?;
    let pct = (total_size / 100).max(1);
    tracing::info!("Checks complete, migrating repo");
    tracing::info!("{total_size} hashes will be migrated");

    let hash_stream = std::pin::pin!(old_repo.hashes());
    let mut hash_stream = hash_stream.into_streamer();

    let mut index = 0;
    while let Some(res) = hash_stream.next().await {
        if let Ok(hash) = res {
            let _ = migrate_hash(old_repo.clone(), new_repo.clone(), hash).await;
        } else {
            tracing::warn!("Failed to read hash, skipping");
        }

        index += 1;

        if index % pct == 0 {
            let percent = index / pct;

            tracing::info!("Migration {percent}% complete - {index}/{total_size}");
        }
    }

    if let Some(generator_state) = old_repo.get(GENERATOR_KEY).await? {
        new_repo
            .set(GENERATOR_KEY, generator_state.to_vec().into())
            .await?;
    }

    tracing::info!("Migration complete");

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(crate) async fn migrate_04<S: Store + 'static>(
    tmp_dir: ArcTmpDir,
    old_repo: OldSledRepo,
    new_repo: ArcRepo,
    store: S,
    config: Configuration,
) -> Result<(), Error> {
    tracing::info!("Running checks");
    if let Err(e) = old_repo.health_check().await {
        tracing::warn!("Old repo is not configured correctly");
        return Err(e.into());
    }
    if let Err(e) = new_repo.health_check().await {
        tracing::warn!("New repo is not configured correctly");
        return Err(e.into());
    }
    if let Err(e) = store.health_check().await {
        tracing::warn!("Store is not configured correctly");
        return Err(e.into());
    }

    let total_size = old_repo.size().await?;
    let pct = (total_size / 100).max(1);
    tracing::info!("Checks complete, migrating repo");
    tracing::info!("{total_size} hashes will be migrated");

    let mut hash_stream = old_repo.hashes().await.into_streamer();

    let mut set = JoinSet::new();

    let mut index = 0;
    while let Some(res) = hash_stream.next().await {
        if let Ok(hash) = res {
            set.spawn_local(migrate_hash_04(
                tmp_dir.clone(),
                old_repo.clone(),
                new_repo.clone(),
                store.clone(),
                config.clone(),
                hash.clone(),
            ));
        } else {
            tracing::warn!("Failed to read hash, skipping");
        }

        while set.len() >= config.upgrade.concurrency {
            if set.join_next().await.is_some() {
                index += 1;

                if index % pct == 0 {
                    let percent = index / pct;

                    tracing::info!("Migration {percent}% complete - {index}/{total_size}");
                }
            }
        }
    }

    while set.join_next().await.is_some() {
        index += 1;

        if index % pct == 0 {
            let percent = index / pct;

            tracing::info!("Migration {percent}% complete - {index}/{total_size}");
        }
    }

    if let Some(generator_state) = old_repo.get(GENERATOR_KEY).await? {
        new_repo
            .set(GENERATOR_KEY, generator_state.to_vec().into())
            .await?;
    }

    if let Some(generator_state) = old_repo.get(crate::NOT_FOUND_KEY).await? {
        new_repo
            .set(crate::NOT_FOUND_KEY, generator_state.to_vec().into())
            .await?;
    }

    tracing::info!("Migration complete");

    Ok(())
}

async fn migrate_hash(old_repo: ArcRepo, new_repo: ArcRepo, hash: Hash) {
    let mut hash_failures = 0;

    while let Err(e) = do_migrate_hash(&old_repo, &new_repo, hash.clone()).await {
        hash_failures += 1;

        if hash_failures > 10 {
            tracing::error!(
                "Failed to migrate hash {}, skipping\n{hash:?}",
                format!("{e:?}")
            );

            break;
        } else {
            tracing::debug!("Failed to migrate hash {hash:?}, retrying +{hash_failures}");
        }
    }
}

async fn migrate_hash_04<S: Store>(
    tmp_dir: ArcTmpDir,
    old_repo: OldSledRepo,
    new_repo: ArcRepo,
    store: S,
    config: Configuration,
    old_hash: sled::IVec,
) {
    let mut hash_failures = 0;

    while let Err(e) = do_migrate_hash_04(
        &tmp_dir,
        &old_repo,
        &new_repo,
        &store,
        &config,
        old_hash.clone(),
    )
    .await
    {
        hash_failures += 1;

        if hash_failures > 10 {
            tracing::error!(
                "Failed to migrate hash {}, skipping\n{}",
                hex::encode(&old_hash[..]),
                format!("{e:?}")
            );

            break;
        } else {
            tracing::debug!(
                "Failed to migrate hash {}, retrying +{hash_failures}",
                hex::encode(&old_hash[..])
            );
        }
    }
}

#[tracing::instrument(skip_all)]
async fn do_migrate_hash(old_repo: &ArcRepo, new_repo: &ArcRepo, hash: Hash) -> Result<(), Error> {
    let Some(identifier) = old_repo.identifier(hash.clone()).await? else {
        tracing::warn!("Skipping hash {hash:?}, no identifier");
        return Ok(());
    };

    if let Some(details) = old_repo.details(&identifier).await? {
        let _ = new_repo
            .create_hash_with_timestamp(hash.clone(), &identifier, details.created_at())
            .await?;

        new_repo.relate_details(&identifier, &details).await?;
    } else {
        let _ = new_repo.create_hash(hash.clone(), &identifier).await?;
    }

    if let Some(identifier) = old_repo.motion_identifier(hash.clone()).await? {
        new_repo
            .relate_motion_identifier(hash.clone(), &identifier)
            .await?;

        if let Some(details) = old_repo.details(&identifier).await? {
            new_repo.relate_details(&identifier, &details).await?;
        }
    }

    for alias in old_repo.aliases_for_hash(hash.clone()).await? {
        let delete_token = old_repo
            .delete_token(&alias)
            .await?
            .unwrap_or_else(DeleteToken::generate);
        let _ = new_repo
            .create_alias(&alias, &delete_token, hash.clone())
            .await?;

        if let Some(timestamp) = old_repo.alias_accessed_at(alias.clone()).await? {
            new_repo.set_accessed_alias(alias, timestamp).await?;
        }
    }

    for (variant, identifier) in old_repo.variants(hash.clone()).await? {
        let _ = new_repo
            .relate_variant_identifier(hash.clone(), variant.clone(), &identifier)
            .await?;

        if let Some(timestamp) = new_repo
            .variant_accessed_at(hash.clone(), variant.clone())
            .await?
        {
            new_repo
                .set_accessed_variant(hash.clone(), variant, timestamp)
                .await?;
        } else {
            new_repo.accessed_variant(hash.clone(), variant).await?;
        }

        if let Some(details) = old_repo.details(&identifier).await? {
            new_repo.relate_details(&identifier, &details).await?;
        }
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
async fn do_migrate_hash_04<S: Store>(
    tmp_dir: &TmpDir,
    old_repo: &OldSledRepo,
    new_repo: &ArcRepo,
    store: &S,
    config: &Configuration,
    old_hash: sled::IVec,
) -> Result<(), Error> {
    let Some(identifier) = old_repo.identifier(old_hash.clone()).await? else {
        tracing::warn!("Skipping hash {}, no identifier", hex::encode(&old_hash));
        return Ok(());
    };

    let size = store.len(&identifier).await?;

    let hash_details = set_details(tmp_dir, old_repo, new_repo, store, config, &identifier).await?;

    let aliases = old_repo.aliases_for_hash(old_hash.clone()).await?;
    let variants = old_repo.variants(old_hash.clone()).await?;
    let motion_identifier = old_repo.motion_identifier(old_hash.clone()).await?;

    let hash = old_hash[..].try_into().expect("Invalid hash size");

    let hash = Hash::new(hash, size, hash_details.internal_format());

    let _ = new_repo
        .create_hash_with_timestamp(hash.clone(), &identifier, hash_details.created_at())
        .await?;

    for alias in aliases {
        let delete_token = old_repo
            .delete_token(&alias)
            .await?
            .unwrap_or_else(DeleteToken::generate);

        let _ = new_repo
            .create_alias(&alias, &delete_token, hash.clone())
            .await?;
    }

    if let Some(identifier) = motion_identifier {
        new_repo
            .relate_motion_identifier(hash.clone(), &identifier)
            .await?;

        set_details(tmp_dir, old_repo, new_repo, store, config, &identifier).await?;
    }

    for (variant, identifier) in variants {
        let _ = new_repo
            .relate_variant_identifier(hash.clone(), variant.clone(), &identifier)
            .await?;

        set_details(tmp_dir, old_repo, new_repo, store, config, &identifier).await?;

        new_repo.accessed_variant(hash.clone(), variant).await?;
    }

    Ok(())
}

async fn set_details<S: Store>(
    tmp_dir: &TmpDir,
    old_repo: &OldSledRepo,
    new_repo: &ArcRepo,
    store: &S,
    config: &Configuration,
    identifier: &Arc<str>,
) -> Result<Details, Error> {
    if let Some(details) = new_repo.details(identifier).await? {
        Ok(details)
    } else {
        let details =
            fetch_or_generate_details(tmp_dir, old_repo, store, config, identifier).await?;
        new_repo.relate_details(identifier, &details).await?;
        Ok(details)
    }
}

static DETAILS_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn details_semaphore() -> &'static Semaphore {
    DETAILS_SEMAPHORE.get_or_init(|| {
        let parallelism = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);

        crate::sync::bare_semaphore(parallelism * 2)
    })
}

#[tracing::instrument(skip_all)]
async fn fetch_or_generate_details<S: Store>(
    tmp_dir: &TmpDir,
    old_repo: &OldSledRepo,
    store: &S,
    config: &Configuration,
    identifier: &Arc<str>,
) -> Result<Details, Error> {
    let details_opt = old_repo.details(identifier.clone()).await?;

    if let Some(details) = details_opt {
        Ok(details)
    } else {
        let bytes_stream = store.to_bytes(identifier, None, None).await?;
        let bytes = bytes_stream.into_bytes();

        let guard = details_semaphore().acquire().await?;
        let details = Details::from_bytes(tmp_dir, config.media.process_timeout, bytes).await?;
        drop(guard);

        Ok(details)
    }
}
