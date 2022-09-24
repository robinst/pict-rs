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
use tracing::{Instrument, Span};

mod hasher;
use hasher::Hasher;

#[derive(Debug)]
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

#[tracing::instrument(name = "Aggregate", skip(stream))]
async fn aggregate<S>(stream: S) -> Result<Bytes, Error>
where
    S: Stream<Item = Result<Bytes, Error>>,
{
    futures_util::pin_mut!(stream);

    let mut buf = Vec::new();
    tracing::debug!("Reading stream to memory");
    while let Some(res) = stream.next().await {
        let bytes = res?;
        buf.push(bytes);
    }

    let total_len = buf.iter().fold(0, |acc, item| acc + item.len());

    let bytes_mut = buf
        .iter()
        .fold(BytesMut::with_capacity(total_len), |mut acc, item| {
            acc.extend_from_slice(item);
            acc
        });

    Ok(bytes_mut.freeze())
}

#[tracing::instrument(name = "Ingest", skip(stream))]
pub(crate) async fn ingest<R, S>(
    repo: &R,
    store: &S,
    stream: impl Stream<Item = Result<Bytes, Error>> + Unpin + 'static,
    declared_alias: Option<Alias>,
    should_validate: bool,
    is_cached: bool,
) -> Result<Session<R, S>, Error>
where
    R: FullRepo + 'static,
    S: Store,
{
    let permit = crate::PROCESS_SEMAPHORE.acquire().await;

    let bytes = aggregate(stream).await?;

    tracing::debug!("Validating bytes");
    let (input_type, validated_reader) = crate::validate::validate_image_bytes(
        bytes,
        CONFIG.media.format,
        CONFIG.media.enable_silent_video,
        should_validate,
    )
    .await?;

    let hasher_reader = Hasher::new(validated_reader, Sha256::new());
    let hasher = hasher_reader.hasher();

    let identifier = store.save_async_read(hasher_reader).await?;

    drop(permit);

    let mut session = Session {
        repo: repo.clone(),
        hash: None,
        alias: None,
        identifier: Some(identifier.clone()),
    };

    let hash = hasher.borrow_mut().finalize_reset().to_vec();

    session.hash = Some(hash.clone());

    save_upload(repo, store, &hash, &identifier).await?;

    if let Some(alias) = declared_alias {
        session.add_existing_alias(&hash, alias, is_cached).await?
    } else {
        session.create_alias(&hash, input_type, is_cached).await?;
    }

    Ok(session)
}

#[tracing::instrument]
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

    #[tracing::instrument]
    pub(crate) async fn delete_token(&self) -> Result<DeleteToken, Error> {
        let alias = self.alias.clone().ok_or(UploadError::MissingAlias)?;

        tracing::debug!("Generating delete token");
        let delete_token = DeleteToken::generate();

        tracing::debug!("Saving delete token");
        let res = self.repo.relate_delete_token(&alias, &delete_token).await?;

        if res.is_err() {
            let delete_token = self.repo.delete_token(&alias).await?;
            tracing::debug!("Returning existing delete token, {:?}", delete_token);
            return Ok(delete_token);
        }

        tracing::debug!("Returning new delete token, {:?}", delete_token);
        Ok(delete_token)
    }

    #[tracing::instrument]
    async fn add_existing_alias(
        &mut self,
        hash: &[u8],
        alias: Alias,
        is_cached: bool,
    ) -> Result<(), Error> {
        AliasRepo::create(&self.repo, &alias)
            .await?
            .map_err(|_| UploadError::DuplicateAlias)?;

        self.alias = Some(alias.clone());

        self.repo.relate_hash(&alias, hash.to_vec().into()).await?;
        self.repo.relate_alias(hash.to_vec().into(), &alias).await?;

        if is_cached {
            self.repo.mark_cached(&alias).await?;
        }

        Ok(())
    }

    #[tracing::instrument]
    async fn create_alias(
        &mut self,
        hash: &[u8],
        input_type: ValidInputType,
        is_cached: bool,
    ) -> Result<(), Error> {
        tracing::debug!("Alias gen loop");

        loop {
            let alias = Alias::generate(input_type.as_ext().to_string());

            if AliasRepo::create(&self.repo, &alias).await?.is_ok() {
                self.alias = Some(alias.clone());

                self.repo.relate_hash(&alias, hash.to_vec().into()).await?;
                self.repo.relate_alias(hash.to_vec().into(), &alias).await?;

                if is_cached {
                    self.repo.mark_cached(&alias).await?;
                }

                return Ok(());
            }

            tracing::debug!("Alias exists, regenerating");
        }
    }
}

impl<R, S> Drop for Session<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    #[tracing::instrument(name = "Drop Session", skip(self), fields(hash = ?self.hash, alias = ?self.alias, identifier = ?self.identifier))]
    fn drop(&mut self) {
        if let Some(hash) = self.hash.take() {
            let repo = self.repo.clone();

            let cleanup_span =
                tracing::info_span!(parent: None, "Session cleanup hash", hash = ?hash);
            cleanup_span.follows_from(Span::current());

            tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
                actix_rt::spawn(
                    async move {
                        let _ = crate::queue::cleanup_hash(&repo, hash.into()).await;
                    }
                    .instrument(cleanup_span),
                )
            });
        }

        if let Some(alias) = self.alias.take() {
            let repo = self.repo.clone();

            let cleanup_span =
                tracing::info_span!(parent: None, "Session cleanup alias", alias = ?alias);
            cleanup_span.follows_from(Span::current());

            tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
                actix_rt::spawn(
                    async move {
                        if let Ok(token) = repo.delete_token(&alias).await {
                            let _ = crate::queue::cleanup_alias(&repo, alias, token).await;
                        } else {
                            let token = DeleteToken::generate();
                            if let Ok(Ok(())) = repo.relate_delete_token(&alias, &token).await {
                                let _ = crate::queue::cleanup_alias(&repo, alias, token).await;
                            }
                        }
                    }
                    .instrument(cleanup_span),
                )
            });
        }

        if let Some(identifier) = self.identifier.take() {
            let repo = self.repo.clone();

            let cleanup_span = tracing::info_span!(parent: None, "Session cleanup identifier", identifier = ?identifier);
            cleanup_span.follows_from(Span::current());

            tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
                actix_rt::spawn(
                    async move {
                        let _ = crate::queue::cleanup_identifier(&repo, identifier).await;
                    }
                    .instrument(cleanup_span),
                )
            });
        }
    }
}
