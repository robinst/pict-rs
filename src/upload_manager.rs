use crate::{
    config::ImageFormat,
    details::Details,
    error::{Error, UploadError},
    ffmpeg::{InputFormat, ThumbnailFormat},
    magick::details_hint,
    repo::{
        sled::SledRepo, Alias, AliasRepo, BaseRepo, DeleteToken, HashRepo, IdentifierRepo, Repo,
        SettingsRepo,
    },
    store::{Identifier, Store},
};
use futures_util::StreamExt;
use sha2::Digest;
use std::sync::Arc;
use tracing::instrument;

mod hasher;
mod session;

pub(super) use session::UploadManagerSession;

const STORE_MIGRATION_PROGRESS: &[u8] = b"store-migration-progress";

#[derive(Clone)]
pub(crate) struct UploadManager {
    inner: Arc<UploadManagerInner>,
}

pub(crate) struct UploadManagerInner {
    format: Option<ImageFormat>,
    hasher: sha2::Sha256,
    repo: Repo,
}

impl UploadManager {
    pub(crate) fn repo(&self) -> &Repo {
        &self.inner.repo
    }

    /// Create a new UploadManager
    pub(crate) async fn new(repo: Repo, format: Option<ImageFormat>) -> Result<Self, Error> {
        let manager = UploadManager {
            inner: Arc::new(UploadManagerInner {
                format,
                hasher: sha2::Sha256::new(),
                repo,
            }),
        };

        Ok(manager)
    }

    pub(crate) async fn migrate_store<S1, S2>(&self, from: S1, to: S2) -> Result<(), Error>
    where
        S1: Store,
        S2: Store,
    {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => do_migrate_store(sled_repo, from, to).await,
        }
    }

    pub(crate) async fn still_identifier_from_alias<S: Store + Clone>(
        &self,
        store: S,
        alias: &Alias,
    ) -> Result<S::Identifier, Error> {
        let identifier = self.identifier_from_alias::<S>(alias).await?;
        let details = if let Some(details) = self.details(&identifier).await? {
            details
        } else {
            let hint = details_hint(alias);
            Details::from_store(store.clone(), identifier.clone(), hint).await?
        };

        if !details.is_motion() {
            return Ok(identifier);
        }

        if let Some(motion_identifier) = self.motion_identifier::<S>(alias).await? {
            return Ok(motion_identifier);
        }

        let permit = crate::PROCESS_SEMAPHORE.acquire().await;
        let mut reader = crate::ffmpeg::thumbnail(
            store.clone(),
            identifier,
            InputFormat::Mp4,
            ThumbnailFormat::Jpeg,
        )
        .await?;
        let motion_identifier = store.save_async_read(&mut reader).await?;
        drop(permit);

        self.store_motion_identifier(alias, &motion_identifier)
            .await?;
        Ok(motion_identifier)
    }

    async fn motion_identifier<S: Store>(
        &self,
        alias: &Alias,
    ) -> Result<Option<S::Identifier>, Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash = sled_repo.hash(alias).await?;
                Ok(sled_repo.motion_identifier(hash).await?)
            }
        }
    }

    async fn store_motion_identifier<I: Identifier + 'static>(
        &self,
        alias: &Alias,
        identifier: &I,
    ) -> Result<(), Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash = sled_repo.hash(alias).await?;
                Ok(sled_repo.relate_motion_identifier(hash, identifier).await?)
            }
        }
    }

    #[instrument(skip(self))]
    pub(crate) async fn identifier_from_alias<S: Store>(
        &self,
        alias: &Alias,
    ) -> Result<S::Identifier, Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash = sled_repo.hash(alias).await?;
                Ok(sled_repo.identifier(hash).await?)
            }
        }
    }

    #[instrument(skip(self))]
    async fn store_identifier<I: Identifier>(
        &self,
        hash: Vec<u8>,
        identifier: &I,
    ) -> Result<(), Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                Ok(sled_repo.relate_identifier(hash.into(), identifier).await?)
            }
        }
    }

    #[instrument(skip(self))]
    pub(crate) async fn variant_identifier<S: Store>(
        &self,
        alias: &Alias,
        process_path: &std::path::Path,
    ) -> Result<Option<S::Identifier>, Error> {
        let variant = process_path.to_string_lossy().to_string();

        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash = sled_repo.hash(alias).await?;
                Ok(sled_repo.variant_identifier(hash, variant).await?)
            }
        }
    }

    /// Store the path to a generated image variant so we can easily clean it up later
    #[instrument(skip(self))]
    pub(crate) async fn store_full_res<I: Identifier>(
        &self,
        alias: &Alias,
        identifier: &I,
    ) -> Result<(), Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash = sled_repo.hash(alias).await?;
                Ok(sled_repo.relate_identifier(hash, identifier).await?)
            }
        }
    }

    /// Store the path to a generated image variant so we can easily clean it up later
    #[instrument(skip(self))]
    pub(crate) async fn store_variant<I: Identifier>(
        &self,
        alias: &Alias,
        variant_process_path: &std::path::Path,
        identifier: &I,
    ) -> Result<(), Error> {
        let variant = variant_process_path.to_string_lossy().to_string();

        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash = sled_repo.hash(alias).await?;
                Ok(sled_repo
                    .relate_variant_identifier(hash, variant, identifier)
                    .await?)
            }
        }
    }

    /// Get the image details for a given variant
    #[instrument(skip(self))]
    pub(crate) async fn details<I: Identifier>(
        &self,
        identifier: &I,
    ) -> Result<Option<Details>, Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => Ok(sled_repo.details(identifier).await?),
        }
    }

    #[instrument(skip(self))]
    pub(crate) async fn store_details<I: Identifier>(
        &self,
        identifier: &I,
        details: &Details,
    ) -> Result<(), Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => Ok(sled_repo.relate_details(identifier, details).await?),
        }
    }

    /// Get a list of aliases for a given alias
    pub(crate) async fn aliases_by_alias(&self, alias: &Alias) -> Result<Vec<Alias>, Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash = sled_repo.hash(alias).await?;
                Ok(sled_repo.aliases(hash).await?)
            }
        }
    }

    /// Delete an alias without a delete token
    pub(crate) async fn delete_without_token(&self, alias: Alias) -> Result<(), Error> {
        let token = match self.inner.repo {
            Repo::Sled(ref sled_repo) => sled_repo.delete_token(&alias).await?,
        };

        self.delete(alias, token).await
    }

    /// Delete the alias, and the file & variants if no more aliases exist
    #[instrument(skip(self, alias, token))]
    pub(crate) async fn delete(&self, alias: Alias, token: DeleteToken) -> Result<(), Error> {
        let hash = match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let saved_delete_token = sled_repo.delete_token(&alias).await?;
                if saved_delete_token != token {
                    return Err(UploadError::InvalidToken.into());
                }
                let hash = sled_repo.hash(&alias).await?;
                AliasRepo::cleanup(sled_repo, &alias).await?;
                sled_repo.remove_alias(hash.clone(), &alias).await?;
                hash.to_vec()
            }
        };

        self.check_delete_files(hash).await
    }

    async fn check_delete_files(&self, hash: Vec<u8>) -> Result<(), Error> {
        match self.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let hash: <SledRepo as BaseRepo>::Bytes = hash.into();

                let aliases = sled_repo.aliases(hash.clone()).await?;

                if !aliases.is_empty() {
                    return Ok(());
                }

                crate::queue::queue_cleanup(sled_repo, hash).await?;
            }
        }

        Ok(())
    }

    pub(crate) fn session<S: Store + Clone + 'static>(&self, store: S) -> UploadManagerSession<S> {
        UploadManagerSession::new(self.clone(), store)
    }
}

async fn migrate_file<S1, S2>(
    from: &S1,
    to: &S2,
    identifier: &S1::Identifier,
) -> Result<S2::Identifier, Error>
where
    S1: Store,
    S2: Store,
{
    let stream = from.to_stream(identifier, None, None).await?;
    futures_util::pin_mut!(stream);
    let mut reader = tokio_util::io::StreamReader::new(stream);

    let new_identifier = to.save_async_read(&mut reader).await?;

    Ok(new_identifier)
}

async fn migrate_details<R, I1, I2>(repo: &R, from: I1, to: &I2) -> Result<(), Error>
where
    R: IdentifierRepo,
    I1: Identifier,
    I2: Identifier,
{
    if let Some(details) = repo.details(&from).await? {
        repo.relate_details(to, &details).await?;
        repo.cleanup(&from).await?;
    }

    Ok(())
}

async fn do_migrate_store<R, S1, S2>(repo: &R, from: S1, to: S2) -> Result<(), Error>
where
    S1: Store,
    S2: Store,
    R: IdentifierRepo + HashRepo + SettingsRepo,
{
    let stream = repo.hashes().await;
    let mut stream = Box::pin(stream);

    while let Some(hash) = stream.next().await {
        let hash = hash?;
        if let Some(identifier) = repo
            .motion_identifier(hash.as_ref().to_vec().into())
            .await?
        {
            let new_identifier = migrate_file(&from, &to, &identifier).await?;
            migrate_details(repo, identifier, &new_identifier).await?;
            repo.relate_motion_identifier(hash.as_ref().to_vec().into(), &new_identifier)
                .await?;
        }

        for (variant, identifier) in repo.variants(hash.as_ref().to_vec().into()).await? {
            let new_identifier = migrate_file(&from, &to, &identifier).await?;
            migrate_details(repo, identifier, &new_identifier).await?;
            repo.relate_variant_identifier(hash.as_ref().to_vec().into(), variant, &new_identifier)
                .await?;
        }

        let identifier = repo.identifier(hash.as_ref().to_vec().into()).await?;
        let new_identifier = migrate_file(&from, &to, &identifier).await?;
        migrate_details(repo, identifier, &new_identifier).await?;
        repo.relate_identifier(hash.as_ref().to_vec().into(), &new_identifier)
            .await?;

        repo.set(STORE_MIGRATION_PROGRESS, hash.as_ref().to_vec().into())
            .await?;
    }

    // clean up the migration key to avoid interfering with future migrations
    repo.remove(STORE_MIGRATION_PROGRESS).await?;

    Ok(())
}

impl std::fmt::Debug for UploadManager {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("UploadManager").finish()
    }
}
