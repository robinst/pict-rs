use crate::{
    error::{Error, UploadError},
    magick::ValidInputType,
    repo::{Alias, AliasRepo, AlreadyExists, DeleteToken, HashRepo, IdentifierRepo, Repo},
    store::Store,
    upload_manager::{
        hasher::{Hash, Hasher},
        UploadManager,
    },
};
use actix_web::web;
use futures_util::stream::{Stream, StreamExt};
use tracing::{debug, instrument, Span};
use tracing_futures::Instrument;

pub(crate) struct UploadManagerSession<S: Store + Clone + 'static> {
    store: S,
    manager: UploadManager,
    alias: Option<Alias>,
    finished: bool,
}

impl<S: Store + Clone + 'static> UploadManagerSession<S> {
    pub(super) fn new(manager: UploadManager, store: S) -> Self {
        UploadManagerSession {
            store,
            manager,
            alias: None,
            finished: false,
        }
    }

    pub(crate) fn succeed(mut self) {
        self.finished = true;
    }

    pub(crate) fn alias(&self) -> Option<&Alias> {
        self.alias.as_ref()
    }
}

impl<S: Store + Clone + 'static> Drop for UploadManagerSession<S> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        if let Some(alias) = self.alias.take() {
            let store = self.store.clone();
            let manager = self.manager.clone();
            let cleanup_span = tracing::info_span!(
                parent: None,
                "Upload cleanup",
                alias = &tracing::field::display(&alias),
            );
            cleanup_span.follows_from(Span::current());
            actix_rt::spawn(
                async move {
                    // undo alias -> hash mapping
                    match manager.inner.repo {
                        Repo::Sled(ref sled_repo) => {
                            if let Ok(hash) = sled_repo.hash(&alias).await {
                                debug!("Clean alias repo");
                                let _ = AliasRepo::cleanup(sled_repo, &alias).await;

                                if let Ok(identifier) = sled_repo.identifier(hash.clone()).await {
                                    debug!("Clean identifier repo");
                                    let _ = IdentifierRepo::cleanup(sled_repo, &identifier).await;

                                    debug!("Remove stored files");
                                    let _ = store.remove(&identifier).await;
                                }
                                debug!("Clean hash repo");
                                let _ = HashRepo::cleanup(sled_repo, hash).await;
                            }
                        }
                    }
                }
                .instrument(cleanup_span),
            );
        }
    }
}

impl<S: Store> UploadManagerSession<S> {
    /// Generate a delete token for an alias
    #[instrument(skip(self))]
    pub(crate) async fn delete_token(&self) -> Result<DeleteToken, Error> {
        let alias = self.alias.clone().ok_or(UploadError::MissingAlias)?;

        debug!("Generating delete token");
        let delete_token = DeleteToken::generate();

        debug!("Saving delete token");
        match self.manager.inner.repo {
            Repo::Sled(ref sled_repo) => {
                let res = sled_repo.relate_delete_token(&alias, &delete_token).await?;

                Ok(if res.is_err() {
                    let delete_token = sled_repo.delete_token(&alias).await?;
                    debug!("Returning existing delete token, {:?}", delete_token);
                    delete_token
                } else {
                    debug!("Returning new delete token, {:?}", delete_token);
                    delete_token
                })
            }
        }
    }

    /// Import the file, discarding bytes if it's already present, or saving if it's new
    pub(crate) async fn import(
        mut self,
        alias: String,
        validate: bool,
        mut stream: impl Stream<Item = Result<web::Bytes, Error>> + Unpin,
    ) -> Result<Self, Error> {
        let mut bytes_mut = actix_web::web::BytesMut::new();

        debug!("Reading stream to memory");
        while let Some(res) = stream.next().await {
            let bytes = res?;
            bytes_mut.extend_from_slice(&bytes);
        }

        debug!("Validating bytes");
        let (_, validated_reader) = crate::validate::validate_image_bytes(
            bytes_mut.freeze(),
            self.manager.inner.format,
            validate,
        )
        .await?;

        let mut hasher_reader = Hasher::new(validated_reader, self.manager.inner.hasher.clone());

        let identifier = self.store.save_async_read(&mut hasher_reader).await?;
        let hash = hasher_reader.finalize_reset().await?;

        debug!("Adding alias");
        self.add_existing_alias(&hash, alias).await?;

        debug!("Saving file");
        self.save_upload(&identifier, hash).await?;

        // Return alias to file
        Ok(self)
    }

    /// Upload the file, discarding bytes if it's already present, or saving if it's new
    #[instrument(skip(self, stream))]
    pub(crate) async fn upload(
        mut self,
        mut stream: impl Stream<Item = Result<web::Bytes, Error>> + Unpin,
    ) -> Result<Self, Error> {
        let mut bytes_mut = actix_web::web::BytesMut::new();

        debug!("Reading stream to memory");
        while let Some(res) = stream.next().await {
            let bytes = res?;
            bytes_mut.extend_from_slice(&bytes);
        }

        debug!("Validating bytes");
        let (input_type, validated_reader) = crate::validate::validate_image_bytes(
            bytes_mut.freeze(),
            self.manager.inner.format,
            true,
        )
        .await?;

        let mut hasher_reader = Hasher::new(validated_reader, self.manager.inner.hasher.clone());

        let identifier = self.store.save_async_read(&mut hasher_reader).await?;
        let hash = hasher_reader.finalize_reset().await?;

        debug!("Adding alias");
        self.add_alias(&hash, input_type).await?;

        debug!("Saving file");
        self.save_upload(&identifier, hash).await?;

        // Return alias to file
        Ok(self)
    }

    // check duplicates & store image if new
    #[instrument(skip(self, hash))]
    async fn save_upload(&self, identifier: &S::Identifier, hash: Hash) -> Result<(), Error> {
        let res = self.check_duplicate(&hash).await?;

        // bail early with alias to existing file if this is a duplicate
        if res.is_err() {
            debug!("Duplicate exists, removing file");

            self.store.remove(identifier).await?;
            return Ok(());
        }

        self.manager
            .store_identifier(hash.into_inner(), identifier)
            .await?;

        Ok(())
    }

    // check for an already-uploaded image with this hash, returning the path to the target file
    #[instrument(skip(self, hash))]
    async fn check_duplicate(&self, hash: &Hash) -> Result<Result<(), AlreadyExists>, Error> {
        let hash = hash.as_slice().to_vec();

        match self.manager.inner.repo {
            Repo::Sled(ref sled_repo) => Ok(HashRepo::create(sled_repo, hash.into()).await?),
        }
    }

    // Add an alias from an existing filename
    async fn add_existing_alias(&mut self, hash: &Hash, filename: String) -> Result<(), Error> {
        let alias = Alias::from_existing(&filename);

        match self.manager.inner.repo {
            Repo::Sled(ref sled_repo) => {
                AliasRepo::create(sled_repo, &alias)
                    .await?
                    .map_err(|_| UploadError::DuplicateAlias)?;
                self.alias = Some(alias.clone());

                let hash = hash.as_slice().to_vec();
                sled_repo.relate_hash(&alias, hash.clone().into()).await?;
                sled_repo.relate_alias(hash.into(), &alias).await?;
            }
        }

        Ok(())
    }

    // Add an alias to an existing file
    //
    // This will help if multiple 'users' upload the same file, and one of them wants to delete it
    #[instrument(skip(self, hash, input_type))]
    async fn add_alias(&mut self, hash: &Hash, input_type: ValidInputType) -> Result<(), Error> {
        loop {
            debug!("Alias gen loop");
            let alias = Alias::generate(input_type.as_ext().to_string());

            match self.manager.inner.repo {
                Repo::Sled(ref sled_repo) => {
                    let res = AliasRepo::create(sled_repo, &alias).await?;

                    if res.is_ok() {
                        self.alias = Some(alias.clone());
                        let hash = hash.as_slice().to_vec();
                        sled_repo.relate_hash(&alias, hash.clone().into()).await?;
                        sled_repo.relate_alias(hash.into(), &alias).await?;
                        return Ok(());
                    }
                }
            };

            debug!("Alias exists, regenning");
        }
    }
}
