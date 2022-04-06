use crate::{
    error::{Error, UploadError},
    magick::ValidInputType,
    repo::{Alias, AliasRepo, DeleteToken, FullRepo, HashRepo},
    store::Store,
    CONFIG,
};
use actix_web::web::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use tracing::debug;

mod hasher;
use hasher::Hasher;

pub(crate) struct Session<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    repo: R,
    hash: Option<Vec<u8>>,
    alias: Option<Alias>,
    identifier: Option<S::Identifier>,
}

pub(crate) async fn ingest<R, S>(
    repo: &R,
    store: &S,
    stream: impl Stream<Item = Result<Bytes, Error>>,
    declared_alias: Option<Alias>,
    should_validate: bool,
) -> Result<Session<R, S>, Error>
where
    R: FullRepo + 'static,
    S: Store,
{
    let permit = crate::PROCESS_SEMAPHORE.acquire().await;

    let mut bytes_mut = BytesMut::new();

    futures_util::pin_mut!(stream);

    debug!("Reading stream to memory");
    while let Some(res) = stream.next().await {
        let bytes = res?;
        bytes_mut.extend_from_slice(&bytes);
    }

    debug!("Validating bytes");
    let (input_type, validated_reader) = crate::validate::validate_image_bytes(
        bytes_mut.freeze(),
        CONFIG.media.format,
        CONFIG.media.enable_silent_video,
        should_validate,
    )
    .await?;

    let mut hasher_reader = Hasher::new(validated_reader, Sha256::new());

    let identifier = store.save_async_read(&mut hasher_reader).await?;

    drop(permit);

    let mut session = Session {
        repo: repo.clone(),
        hash: None,
        alias: None,
        identifier: Some(identifier.clone()),
    };

    let hash = hasher_reader.finalize_reset().await?;

    session.hash = Some(hash.clone());

    debug!("Saving upload");

    save_upload(repo, store, &hash, &identifier).await?;

    debug!("Adding alias");

    if let Some(alias) = declared_alias {
        session.add_existing_alias(&hash, alias).await?
    } else {
        session.create_alias(&hash, input_type).await?;
    }

    Ok(session)
}

async fn save_upload<R, S>(
    repo: &R,
    store: &S,
    hash: &[u8],
    identifier: &S::Identifier,
) -> Result<(), Error>
where
    S: Store,
    R: FullRepo,
{
    if HashRepo::create(repo, hash.to_vec().into()).await?.is_err() {
        store.remove(identifier).await?;
        return Ok(());
    }

    repo.relate_identifier(hash.to_vec().into(), identifier)
        .await?;

    Ok(())
}

impl<R, S> Session<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    pub(crate) fn disarm(&mut self) {
        let _ = self.hash.take();
        let _ = self.alias.take();
        let _ = self.identifier.take();
    }

    pub(crate) fn alias(&self) -> Option<&Alias> {
        self.alias.as_ref()
    }

    pub(crate) async fn delete_token(&self) -> Result<DeleteToken, Error> {
        let alias = self.alias.clone().ok_or(UploadError::MissingAlias)?;

        debug!("Generating delete token");
        let delete_token = DeleteToken::generate();

        debug!("Saving delete token");
        let res = self.repo.relate_delete_token(&alias, &delete_token).await?;

        if res.is_err() {
            let delete_token = self.repo.delete_token(&alias).await?;
            debug!("Returning existing delete token, {:?}", delete_token);
            return Ok(delete_token);
        }

        debug!("Returning new delete token, {:?}", delete_token);
        Ok(delete_token)
    }

    async fn add_existing_alias(&mut self, hash: &[u8], alias: Alias) -> Result<(), Error> {
        AliasRepo::create(&self.repo, &alias)
            .await?
            .map_err(|_| UploadError::DuplicateAlias)?;

        self.alias = Some(alias.clone());

        self.repo.relate_hash(&alias, hash.to_vec().into()).await?;
        self.repo.relate_alias(hash.to_vec().into(), &alias).await?;

        Ok(())
    }

    async fn create_alias(&mut self, hash: &[u8], input_type: ValidInputType) -> Result<(), Error> {
        debug!("Alias gen loop");

        loop {
            let alias = Alias::generate(input_type.as_ext().to_string());

            if AliasRepo::create(&self.repo, &alias).await?.is_ok() {
                self.alias = Some(alias.clone());

                self.repo.relate_hash(&alias, hash.to_vec().into()).await?;
                self.repo.relate_alias(hash.to_vec().into(), &alias).await?;

                return Ok(());
            }

            debug!("Alias exists, regenerating");
        }
    }
}

impl<R, S> Drop for Session<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    fn drop(&mut self) {
        if let Some(hash) = self.hash.take() {
            let repo = self.repo.clone();
            actix_rt::spawn(async move {
                let _ = crate::queue::cleanup_hash(&repo, hash.into()).await;
            });
        }

        if let Some(alias) = self.alias.take() {
            let repo = self.repo.clone();

            actix_rt::spawn(async move {
                if let Ok(token) = repo.delete_token(&alias).await {
                    let _ = crate::queue::cleanup_alias(&repo, alias, token).await;
                } else {
                    let token = DeleteToken::generate();
                    if let Ok(Ok(())) = repo.relate_delete_token(&alias, &token).await {
                        let _ = crate::queue::cleanup_alias(&repo, alias, token).await;
                    }
                }
            });
        }

        if let Some(identifier) = self.identifier.take() {
            let repo = self.repo.clone();

            actix_rt::spawn(async move {
                let _ = crate::queue::cleanup_identifier(&repo, identifier).await;
            });
        }
    }
}
