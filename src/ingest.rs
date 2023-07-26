use crate::{
    bytes_stream::BytesStream,
    either::Either,
    error::{Error, UploadError},
    formats::{InternalFormat, Validations},
    repo::{Alias, AliasRepo, DeleteToken, FullRepo, HashRepo},
    store::Store,
};
use actix_web::web::Bytes;
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
    delete_token: DeleteToken,
    hash: Option<Vec<u8>>,
    alias: Option<Alias>,
    identifier: Option<S::Identifier>,
}

#[tracing::instrument(skip(stream))]
async fn aggregate<S>(mut stream: S) -> Result<Bytes, Error>
where
    S: Stream<Item = Result<Bytes, Error>> + Unpin,
{
    let mut buf = BytesStream::new();

    while let Some(res) = stream.next().await {
        buf.add_bytes(res?);
    }

    Ok(buf.into_bytes())
}

#[tracing::instrument(skip(repo, store, stream, media))]
pub(crate) async fn ingest<R, S>(
    repo: &R,
    store: &S,
    stream: impl Stream<Item = Result<Bytes, Error>> + Unpin + 'static,
    declared_alias: Option<Alias>,
    media: &crate::config::Media,
) -> Result<Session<R, S>, Error>
where
    R: FullRepo + 'static,
    S: Store,
{
    let permit = crate::PROCESS_SEMAPHORE.acquire().await;

    let bytes = aggregate(stream).await?;

    let prescribed = Validations {
        image: &media.image,
        animation: &media.animation,
        video: &media.video,
    };

    tracing::trace!("Validating bytes");
    let (input_type, validated_reader) = crate::validate::validate_bytes(bytes, prescribed).await?;

    let processed_reader = if let Some(operations) = media.preprocess_steps() {
        if let Some(format) = input_type.processable_format() {
            let (_, magick_args) =
                crate::processor::build_chain(operations, format.file_extension())?;

            let quality = match format {
                crate::formats::ProcessableFormat::Image(format) => media.image.quality_for(format),
                crate::formats::ProcessableFormat::Animation(format) => {
                    media.animation.quality_for(format)
                }
            };

            let processed_reader = crate::magick::process_image_async_read(
                validated_reader,
                magick_args,
                format,
                format,
                quality,
            )
            .await?;

            Either::left(processed_reader)
        } else {
            Either::right(validated_reader)
        }
    } else {
        Either::right(validated_reader)
    };

    let hasher_reader = Hasher::new(processed_reader, Sha256::new());
    let hasher = hasher_reader.hasher();

    let identifier = store
        .save_async_read(hasher_reader, input_type.media_type())
        .await?;

    drop(permit);

    let mut session = Session {
        repo: repo.clone(),
        delete_token: DeleteToken::generate(),
        hash: None,
        alias: None,
        identifier: Some(identifier.clone()),
    };

    let hash = hasher.borrow_mut().finalize_reset().to_vec();

    save_upload(&mut session, repo, store, &hash, &identifier).await?;

    if let Some(alias) = declared_alias {
        session.add_existing_alias(&hash, alias).await?
    } else {
        session.create_alias(&hash, input_type).await?
    };

    Ok(session)
}

#[tracing::instrument(level = "trace", skip_all)]
async fn save_upload<R, S>(
    session: &mut Session<R, S>,
    repo: &R,
    store: &S,
    hash: &[u8],
    identifier: &S::Identifier,
) -> Result<(), Error>
where
    S: Store,
    R: FullRepo,
{
    if HashRepo::create(repo, hash.to_vec().into(), identifier)
        .await?
        .is_err()
    {
        // duplicate upload
        store.remove(identifier).await?;
        session.identifier.take();
        return Ok(());
    }

    // Set hash after upload uniquness check so we don't clean existing files on failure
    session.hash = Some(Vec::from(hash));

    Ok(())
}

impl<R, S> Session<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    pub(crate) fn disarm(mut self) -> DeleteToken {
        let _ = self.hash.take();
        let _ = self.alias.take();
        let _ = self.identifier.take();

        self.delete_token.clone()
    }

    pub(crate) fn alias(&self) -> Option<&Alias> {
        self.alias.as_ref()
    }

    pub(crate) fn delete_token(&self) -> &DeleteToken {
        &self.delete_token
    }

    #[tracing::instrument(skip(self, hash))]
    async fn add_existing_alias(&mut self, hash: &[u8], alias: Alias) -> Result<(), Error> {
        let hash: R::Bytes = hash.to_vec().into();

        AliasRepo::create(&self.repo, &alias, &self.delete_token, hash.clone())
            .await?
            .map_err(|_| UploadError::DuplicateAlias)?;

        self.alias = Some(alias.clone());

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self, hash))]
    async fn create_alias(&mut self, hash: &[u8], input_type: InternalFormat) -> Result<(), Error> {
        let hash: R::Bytes = hash.to_vec().into();

        loop {
            let alias = Alias::generate(input_type.file_extension().to_string());

            if AliasRepo::create(&self.repo, &alias, &self.delete_token, hash.clone())
                .await?
                .is_ok()
            {
                self.alias = Some(alias.clone());

                return Ok(());
            }

            tracing::trace!("Alias exists, regenerating");
        }
    }
}

impl<R, S> Drop for Session<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    fn drop(&mut self) {
        let any_items = self.hash.is_some() || self.alias.is_some() || self.identifier.is_some();

        metrics::increment_counter!("pict-rs.ingest.end", "completed" => (!any_items).to_string());

        if self.hash.is_some() || self.alias.is_some() | self.identifier.is_some() {
            let cleanup_parent_span = tracing::info_span!(parent: None, "Dropped session cleanup");
            cleanup_parent_span.follows_from(Span::current());

            if let Some(hash) = self.hash.take() {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Session cleanup hash", hash = ?hash);

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
                let token = self.delete_token.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Session cleanup alias", alias = ?alias);

                tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
                    actix_rt::spawn(
                        async move {
                            let _ = crate::queue::cleanup_alias(&repo, alias, token).await;
                        }
                        .instrument(cleanup_span),
                    )
                });
            }

            if let Some(identifier) = self.identifier.take() {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Session cleanup identifier", identifier = ?identifier);

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
}
