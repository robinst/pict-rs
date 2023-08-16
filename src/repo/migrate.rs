use crate::{
    config::Configuration,
    details::Details,
    error::Error,
    repo::{AliasRepo, ArcRepo, DeleteToken, Hash, HashRepo, VariantAccessRepo},
    repo_04::{
        AliasRepo as _, HashRepo as _, IdentifierRepo as _, SettingsRepo as _,
        SledRepo as OldSledRepo,
    },
    store::Store,
    stream::IntoStreamer,
};

pub(crate) async fn migrate_04<S: Store>(
    old_repo: OldSledRepo,
    new_repo: &ArcRepo,
    store: &S,
    config: &Configuration,
) -> Result<(), Error> {
    tracing::warn!("Running checks");
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

    tracing::warn!("Checks complete, migrating repo");
    tracing::warn!("{} hashes will be migrated", old_repo.size().await?);

    let mut hash_stream = old_repo.hashes().await.into_streamer();

    while let Some(res) = hash_stream.next().await {
        if let Ok(hash) = res {
            let _ = migrate_hash_04(&old_repo, new_repo, store, config, hash).await;
        } else {
            tracing::warn!("Failed to read hash, skipping");
        }
    }

    if let Some(generator_state) = old_repo.get("last-path").await? {
        new_repo
            .set("last-path", generator_state.to_vec().into())
            .await?;
    }

    Ok(())
}

async fn migrate_hash_04<S: Store>(
    old_repo: &OldSledRepo,
    new_repo: &ArcRepo,
    store: &S,
    config: &Configuration,
    old_hash: sled::IVec,
) -> Result<(), Error> {
    let mut hash_failures = 0;

    while let Err(e) = do_migrate_hash_04(old_repo, new_repo, store, config, old_hash.clone()).await
    {
        hash_failures += 1;

        if hash_failures > 10 {
            tracing::error!(
                "Failed to migrate hash {}, skipping\n{}",
                hex::encode(&old_hash[..]),
                format!("{e:?}")
            );
            return Err(e);
        } else {
            tracing::warn!(
                "Failed to migrate hash {}, retrying +1",
                hex::encode(&old_hash[..])
            );
        }
    }

    Ok(())
}

async fn do_migrate_hash_04<S: Store>(
    old_repo: &OldSledRepo,
    new_repo: &ArcRepo,
    store: &S,
    config: &Configuration,
    old_hash: sled::IVec,
) -> Result<(), Error> {
    let Some(identifier) = old_repo.identifier::<S::Identifier>(old_hash.clone()).await? else {
        tracing::warn!("Skipping hash {}, no identifier", hex::encode(&old_hash));
        return Ok(());
    };

    let size = store.len(&identifier).await?;

    let hash_details = details(old_repo, store, config, &identifier).await?;
    let aliases = old_repo.for_hash(old_hash.clone()).await?;
    let variants = old_repo.variants::<S::Identifier>(old_hash.clone()).await?;
    let motion_identifier = old_repo
        .motion_identifier::<S::Identifier>(old_hash.clone())
        .await?;

    let hash = old_hash[..].try_into().expect("Invalid hash size");

    let hash = Hash::new(
        hash,
        size,
        hash_details.internal_format().expect("format exists"),
    );

    HashRepo::create(new_repo.as_ref(), hash.clone(), &identifier)
        .await?
        .expect("not duplicate");
    new_repo.relate_details(&identifier, &hash_details).await?;

    for alias in aliases {
        let delete_token = old_repo
            .delete_token(&alias)
            .await?
            .unwrap_or_else(DeleteToken::generate);

        AliasRepo::create(new_repo.as_ref(), &alias, &delete_token, hash.clone())
            .await?
            .expect("not duplicate");
    }

    if let Some(motion_identifier) = motion_identifier {
        let motion_details = details(old_repo, store, config, &motion_identifier).await?;

        new_repo
            .relate_motion_identifier(hash.clone(), &motion_identifier)
            .await?;
        new_repo
            .relate_details(&motion_identifier, &motion_details)
            .await?;
    }

    for (variant, identifier) in variants {
        let variant_details = details(old_repo, store, config, &identifier).await?;

        new_repo
            .relate_variant_identifier(hash.clone(), variant.clone(), &identifier)
            .await?;
        new_repo
            .relate_details(&identifier, &variant_details)
            .await?;

        VariantAccessRepo::accessed(new_repo.as_ref(), hash.clone(), variant).await?;
    }

    Ok(())
}

async fn details<S: Store>(
    old_repo: &OldSledRepo,
    store: &S,
    config: &Configuration,
    identifier: &S::Identifier,
) -> Result<Details, Error> {
    let details_opt = old_repo.details(identifier).await?.and_then(|details| {
        if details.internal_format().is_some() {
            Some(details)
        } else {
            None
        }
    });

    if let Some(details) = details_opt {
        Ok(details)
    } else {
        Details::from_store(store, identifier, config.media.process_timeout)
            .await
            .map_err(From::from)
    }
}
