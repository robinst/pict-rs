use std::{
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use streem::IntoStreamer;

use crate::{
    details::Details,
    error::{Error, UploadError},
    magick::{ArcPolicyDir, PolicyDir},
    repo::{ArcRepo, Hash},
    store::Store,
    tmp_file::{ArcTmpDir, TmpDir},
};

#[allow(clippy::too_many_arguments)]
pub(super) async fn migrate_store<S1, S2>(
    tmp_dir: ArcTmpDir,
    policy_dir: ArcPolicyDir,
    repo: ArcRepo,
    from: S1,
    to: S2,
    skip_missing_files: bool,
    timeout: u64,
    concurrency: usize,
) -> Result<(), Error>
where
    S1: Store + Clone + 'static,
    S2: Store + Clone + 'static,
{
    tracing::warn!("Running checks");

    if let Err(e) = from.health_check().await {
        tracing::warn!("Old store is not configured correctly");
        return Err(e.into());
    }
    if let Err(e) = to.health_check().await {
        tracing::warn!("New store is not configured correctly");
        return Err(e.into());
    }

    tracing::warn!("Checks complete, migrating store");

    let mut failure_count = 0;

    while let Err(e) = do_migrate_store(
        tmp_dir.clone(),
        policy_dir.clone(),
        repo.clone(),
        from.clone(),
        to.clone(),
        skip_missing_files,
        timeout,
        concurrency,
    )
    .await
    {
        tracing::error!("Migration failed with {}", format!("{e:?}"));

        failure_count += 1;

        if failure_count >= 50 {
            tracing::error!("Exceeded 50 errors");
            return Err(e);
        } else {
            tracing::warn!("Retrying migration +{failure_count}");
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    Ok(())
}

struct MigrateState<S1, S2> {
    tmp_dir: ArcTmpDir,
    policy_dir: ArcPolicyDir,
    repo: ArcRepo,
    from: S1,
    to: S2,
    continuing_migration: bool,
    skip_missing_files: bool,
    initial_repo_size: u64,
    repo_size: AtomicU64,
    pct: AtomicU64,
    index: AtomicU64,
    started_at: Instant,
    timeout: u64,
}

#[allow(clippy::too_many_arguments)]
async fn do_migrate_store<S1, S2>(
    tmp_dir: ArcTmpDir,
    policy_dir: ArcPolicyDir,
    repo: ArcRepo,
    from: S1,
    to: S2,
    skip_missing_files: bool,
    timeout: u64,
    concurrency: usize,
) -> Result<(), Error>
where
    S1: Store + 'static,
    S2: Store + 'static,
{
    let continuing_migration = repo.is_continuing_migration().await?;
    let initial_repo_size = repo.size().await?;

    if continuing_migration {
        tracing::warn!("Continuing previous migration of {initial_repo_size} total hashes");
    } else {
        tracing::warn!("{initial_repo_size} hashes will be migrated");
    }

    if initial_repo_size == 0 {
        return Ok(());
    }

    // Hashes are read in a consistent order
    let stream = std::pin::pin!(repo.hashes());
    let mut stream = stream.into_streamer();

    let state = Rc::new(MigrateState {
        tmp_dir: tmp_dir.clone(),
        policy_dir: policy_dir.clone(),
        repo: repo.clone(),
        from,
        to,
        continuing_migration,
        skip_missing_files,
        initial_repo_size,
        repo_size: AtomicU64::new(initial_repo_size),
        pct: AtomicU64::new(initial_repo_size / 100),
        index: AtomicU64::new(0),
        started_at: Instant::now(),
        timeout,
    });

    let mut joinset = tokio::task::JoinSet::new();

    while let Some(hash) = stream.next().await {
        tracing::trace!("do_migrate_store: looping");

        let hash = hash?;

        if joinset.len() >= concurrency {
            if let Some(res) = joinset.join_next().await {
                res.map_err(|_| UploadError::Canceled)??;
            }
        }

        let state = Rc::clone(&state);
        joinset.spawn_local(async move { migrate_hash(&state, hash).await });
    }

    while let Some(res) = joinset.join_next().await {
        tracing::trace!("do_migrate_store: join looping");

        res.map_err(|_| UploadError::Canceled)??;
    }

    // clean up the migration table to avoid interfering with future migrations
    repo.clear().await?;

    tracing::warn!("Migration completed successfully");

    Ok(())
}

#[tracing::instrument(skip(state))]
async fn migrate_hash<S1, S2>(state: &MigrateState<S1, S2>, hash: Hash) -> Result<(), Error>
where
    S1: Store,
    S2: Store,
{
    let MigrateState {
        tmp_dir,
        policy_dir,
        repo,
        from,
        to,
        continuing_migration,
        skip_missing_files,
        initial_repo_size,
        repo_size,
        pct,
        index,
        started_at,
        timeout,
    } = state;

    let current_index = index.fetch_add(1, Ordering::Relaxed);

    let original_identifier = match repo.identifier(hash.clone()).await {
        Ok(Some(identifier)) => identifier,
        Ok(None) => {
            tracing::warn!(
                "Original File identifier for hash {hash:?} is missing, queue cleanup task",
            );
            crate::queue::cleanup_hash(repo, hash.clone()).await?;
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    if repo.is_migrated(&original_identifier).await? {
        // migrated original for hash - this means we can skip
        return Ok(());
    }

    let current_repo_size = repo_size.load(Ordering::Acquire);

    if *continuing_migration && current_repo_size == *initial_repo_size {
        // first time reaching unmigrated hash

        let new_repo_size = initial_repo_size.saturating_sub(current_index);

        if repo_size
            .compare_exchange(
                current_repo_size,
                new_repo_size,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            // we successfully updated the count, we're now in charge of setting up pct and
            // index and printing migration message

            pct.store(new_repo_size / 100, Ordering::Release);
            index.store(0, Ordering::Release);

            tracing::warn!(
                "Caught up to previous migration's end. {new_repo_size} hashes will be migrated"
            );
        }
    }

    if let Some(identifier) = repo.motion_identifier(hash.clone()).await? {
        if !repo.is_migrated(&identifier).await? {
            match migrate_file(
                tmp_dir,
                policy_dir,
                repo,
                from,
                to,
                &identifier,
                *skip_missing_files,
                *timeout,
            )
            .await
            {
                Ok(new_identifier) => {
                    migrate_details(repo, &identifier, &new_identifier).await?;
                    repo.relate_motion_identifier(hash.clone(), &new_identifier)
                        .await?;

                    repo.mark_migrated(&identifier, &new_identifier).await?;
                }
                Err(MigrateError::From(e)) if e.is_not_found() && *skip_missing_files => {
                    tracing::warn!("Skipping motion file for hash {hash:?}");
                }
                Err(MigrateError::Details(e)) => {
                    tracing::warn!("Error generating details for motion file for hash {hash:?}");
                    return Err(e);
                }
                Err(MigrateError::From(e)) => {
                    tracing::warn!("Error migrating motion file from old store");
                    return Err(e.into());
                }
                Err(MigrateError::To(e)) => {
                    tracing::warn!("Error migrating motion file to new store");
                    return Err(e.into());
                }
            }
        }
    }

    for (variant, identifier) in repo.variants(hash.clone()).await? {
        if !repo.is_migrated(&identifier).await? {
            match migrate_file(
                tmp_dir,
                policy_dir,
                repo,
                from,
                to,
                &identifier,
                *skip_missing_files,
                *timeout,
            )
            .await
            {
                Ok(new_identifier) => {
                    migrate_details(repo, &identifier, &new_identifier).await?;
                    repo.remove_variant(hash.clone(), variant.clone()).await?;
                    let _ = repo
                        .relate_variant_identifier(hash.clone(), variant, &new_identifier)
                        .await?;

                    repo.mark_migrated(&identifier, &new_identifier).await?;
                }
                Err(MigrateError::From(e)) if e.is_not_found() && *skip_missing_files => {
                    tracing::warn!("Skipping variant {variant} for hash {hash:?}",);
                }
                Err(MigrateError::Details(e)) => {
                    tracing::warn!("Error generating details for motion file for hash {hash:?}",);
                    return Err(e);
                }
                Err(MigrateError::From(e)) => {
                    tracing::warn!("Error migrating variant file from old store");
                    return Err(e.into());
                }
                Err(MigrateError::To(e)) => {
                    tracing::warn!("Error migrating variant file to new store");
                    return Err(e.into());
                }
            }
        }
    }

    match migrate_file(
        tmp_dir,
        policy_dir,
        repo,
        from,
        to,
        &original_identifier,
        *skip_missing_files,
        *timeout,
    )
    .await
    {
        Ok(new_identifier) => {
            migrate_details(repo, &original_identifier, &new_identifier).await?;
            repo.update_identifier(hash.clone(), &new_identifier)
                .await?;
            repo.mark_migrated(&original_identifier, &new_identifier)
                .await?;
        }
        Err(MigrateError::From(e)) if e.is_not_found() && *skip_missing_files => {
            tracing::warn!("Skipping original file for hash {hash:?}");
        }
        Err(MigrateError::Details(e)) => {
            tracing::warn!("Error generating details for motion file for hash {hash:?}",);
            return Err(e);
        }
        Err(MigrateError::From(e)) => {
            tracing::warn!("Error migrating original file from old store");
            return Err(e.into());
        }
        Err(MigrateError::To(e)) => {
            tracing::warn!("Error migrating original file to new store");
            return Err(e.into());
        }
    }

    let current_pct = pct.load(Ordering::Relaxed);
    if current_pct > 0 && current_index % current_pct == 0 {
        let percent = u32::try_from(current_index / current_pct)
            .expect("values 0-100 are always in u32 range");
        if percent == 0 {
            return Ok(());
        }

        let elapsed = started_at.elapsed();
        let estimated_duration_percent = elapsed / percent;
        let estimated_duration_remaining =
            (100u32.saturating_sub(percent)) * estimated_duration_percent;

        let current_repo_size = repo_size.load(Ordering::Relaxed);

        tracing::warn!(
            "Migrated {percent}% of hashes ({current_index}/{current_repo_size} total hashes)"
        );
        tracing::warn!("ETA: {estimated_duration_remaining:?} from now");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn migrate_file<S1, S2>(
    tmp_dir: &TmpDir,
    policy_dir: &PolicyDir,
    repo: &ArcRepo,
    from: &S1,
    to: &S2,
    identifier: &Arc<str>,
    skip_missing_files: bool,
    timeout: u64,
) -> Result<Arc<str>, MigrateError>
where
    S1: Store,
    S2: Store,
{
    let mut failure_count = 0;

    loop {
        tracing::trace!("migrate_file: looping");

        match do_migrate_file(tmp_dir, policy_dir, repo, from, to, identifier, timeout).await {
            Ok(identifier) => return Ok(identifier),
            Err(MigrateError::From(e)) if e.is_not_found() && skip_missing_files => {
                return Err(MigrateError::From(e));
            }
            Err(migrate_error) => {
                failure_count += 1;

                if failure_count > 10 {
                    tracing::error!("Error migrating file, not retrying");
                    return Err(migrate_error);
                } else {
                    tracing::warn!("Failed moving file. Retrying +{failure_count}");
                }

                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

#[derive(Debug)]
enum MigrateError {
    From(crate::store::StoreError),
    Details(crate::error::Error),
    To(crate::store::StoreError),
}

async fn do_migrate_file<S1, S2>(
    tmp_dir: &TmpDir,
    policy_dir: &PolicyDir,
    repo: &ArcRepo,
    from: &S1,
    to: &S2,
    identifier: &Arc<str>,
    timeout: u64,
) -> Result<Arc<str>, MigrateError>
where
    S1: Store,
    S2: Store,
{
    let stream = from
        .to_stream(identifier, None, None)
        .await
        .map_err(MigrateError::From)?;

    let details_opt = repo
        .details(identifier)
        .await
        .map_err(Error::from)
        .map_err(MigrateError::Details)?;

    let details = if let Some(details) = details_opt {
        details
    } else {
        let bytes_stream = from
            .to_bytes(identifier, None, None)
            .await
            .map_err(From::from)
            .map_err(MigrateError::Details)?;
        let new_details =
            Details::from_bytes(tmp_dir, policy_dir, timeout, bytes_stream.into_bytes())
                .await
                .map_err(MigrateError::Details)?;
        repo.relate_details(identifier, &new_details)
            .await
            .map_err(Error::from)
            .map_err(MigrateError::Details)?;
        new_details
    };

    let new_identifier = to
        .save_stream(stream, details.media_type())
        .await
        .map_err(MigrateError::To)?;

    Ok(new_identifier)
}

async fn migrate_details(repo: &ArcRepo, from: &Arc<str>, to: &Arc<str>) -> Result<(), Error> {
    if let Some(details) = repo.details(from).await? {
        repo.relate_details(to, &details).await?;
        repo.cleanup_details(from).await?;
    }

    Ok(())
}
