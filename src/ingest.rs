use std::{sync::Arc, time::Duration};

use crate::{
    bytes_stream::BytesStream,
    either::Either,
    error::{Error, UploadError},
    formats::{InternalFormat, Validations},
    future::WithMetrics,
    repo::{Alias, ArcRepo, DeleteToken, Hash},
    store::Store,
};
use actix_web::web::Bytes;
use futures_core::Stream;
use reqwest::Body;
use reqwest_middleware::ClientWithMiddleware;
use streem::IntoStreamer;
use tracing::{Instrument, Span};

mod hasher;
use hasher::Hasher;

#[derive(Debug)]
pub(crate) struct Session {
    repo: ArcRepo,
    delete_token: DeleteToken,
    hash: Option<Hash>,
    alias: Option<Alias>,
    identifier: Option<Arc<str>>,
}

#[tracing::instrument(skip(stream))]
async fn aggregate<S>(stream: S) -> Result<Bytes, Error>
where
    S: Stream<Item = Result<Bytes, Error>>,
{
    let mut buf = BytesStream::new();

    let stream = std::pin::pin!(stream);
    let mut stream = stream.into_streamer();

    while let Some(res) = stream.next().await {
        buf.add_bytes(res?);
    }

    Ok(buf.into_bytes())
}

#[tracing::instrument(skip(repo, store, client, stream, media))]
pub(crate) async fn ingest<S>(
    repo: &ArcRepo,
    store: &S,
    client: &ClientWithMiddleware,
    stream: impl Stream<Item = Result<Bytes, Error>> + 'static,
    declared_alias: Option<Alias>,
    media: &crate::config::Media,
) -> Result<Session, Error>
where
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
    let (input_type, validated_reader) =
        crate::validate::validate_bytes(bytes, prescribed, media.process_timeout).await?;

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
                media.process_timeout,
            )
            .await?;

            Either::left(processed_reader)
        } else {
            Either::right(validated_reader)
        }
    } else {
        Either::right(validated_reader)
    };

    let hasher_reader = Hasher::new(processed_reader);
    let state = hasher_reader.state();

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

    if let Some(endpoint) = &media.external_validation {
        let stream = store.to_stream(&identifier, None, None).await?;

        let response = client
            .post(endpoint.as_str())
            .timeout(Duration::from_secs(media.external_validation_timeout))
            .header("Content-Type", input_type.media_type().as_ref())
            .body(Body::wrap_stream(crate::stream::make_send(stream)))
            .send()
            .instrument(tracing::info_span!("external-validation"))
            .with_metrics("pict-rs.ingest.external-validation")
            .await?;

        if !response.status().is_success() {
            return Err(UploadError::FailedExternalValidation.into());
        }
    }

    let (hash, size) = state.borrow_mut().finalize_reset();

    let hash = Hash::new(hash, size, input_type);

    save_upload(&mut session, repo, store, hash.clone(), &identifier).await?;

    if let Some(alias) = declared_alias {
        session.add_existing_alias(hash, alias).await?
    } else {
        session.create_alias(hash, input_type).await?
    };

    Ok(session)
}

#[tracing::instrument(level = "trace", skip_all)]
async fn save_upload<S>(
    session: &mut Session,
    repo: &ArcRepo,
    store: &S,
    hash: Hash,
    identifier: &Arc<str>,
) -> Result<(), Error>
where
    S: Store,
{
    if repo.create_hash(hash.clone(), identifier).await?.is_err() {
        // duplicate upload
        store.remove(identifier).await?;
        session.identifier.take();
        return Ok(());
    }

    // Set hash after upload uniquness check so we don't clean existing files on failure
    session.hash = Some(hash);

    Ok(())
}

impl Session {
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
    async fn add_existing_alias(&mut self, hash: Hash, alias: Alias) -> Result<(), Error> {
        self.repo
            .create_alias(&alias, &self.delete_token, hash)
            .await?
            .map_err(|_| UploadError::DuplicateAlias)?;

        self.alias = Some(alias.clone());

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self, hash))]
    async fn create_alias(&mut self, hash: Hash, input_type: InternalFormat) -> Result<(), Error> {
        loop {
            let alias = Alias::generate(input_type.file_extension().to_string());

            if self
                .repo
                .create_alias(&alias, &self.delete_token, hash.clone())
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

impl Drop for Session {
    fn drop(&mut self) {
        let any_items = self.hash.is_some() || self.alias.is_some() || self.identifier.is_some();

        metrics::increment_counter!("pict-rs.ingest.end", "completed" => (!any_items).to_string());

        if self.hash.is_some() || self.alias.is_some() | self.identifier.is_some() {
            let cleanup_parent_span = tracing::info_span!(parent: None, "Dropped session cleanup");
            cleanup_parent_span.follows_from(Span::current());

            if let Some(hash) = self.hash.take() {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Session cleanup hash", hash = ?hash);

                crate::sync::spawn(
                    async move {
                        let _ = crate::queue::cleanup_hash(&repo, hash).await;
                    }
                    .instrument(cleanup_span),
                );
            }

            if let Some(alias) = self.alias.take() {
                let repo = self.repo.clone();
                let token = self.delete_token.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Session cleanup alias", alias = ?alias);

                crate::sync::spawn(
                    async move {
                        let _ = crate::queue::cleanup_alias(&repo, alias, token).await;
                    }
                    .instrument(cleanup_span),
                );
            }

            if let Some(identifier) = self.identifier.take() {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Session cleanup identifier", identifier = ?identifier);

                crate::sync::spawn(
                    async move {
                        let _ = crate::queue::cleanup_identifier(&repo, &identifier).await;
                    }
                    .instrument(cleanup_span),
                );
            }
        }
    }
}
