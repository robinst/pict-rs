use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use crate::{
    bytes_stream::BytesStream,
    details::Details,
    error::{Error, UploadError},
    formats::{InternalFormat, Validations},
    future::WithMetrics,
    repo::{Alias, ArcRepo, DeleteToken, Hash},
    store::Store,
    tmp_file::TmpDir,
};
use actix_web::web::Bytes;
use futures_core::Stream;
use reqwest::Body;
use reqwest_middleware::ClientWithMiddleware;
use streem::IntoStreamer;
use tracing::{Instrument, Span};

mod hasher;
use hasher::{Hasher, State};

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

async fn process_ingest<S>(
    tmp_dir: &TmpDir,
    store: &S,
    stream: impl Stream<Item = Result<Bytes, Error>> + 'static,
    media: &crate::config::Media,
) -> Result<(InternalFormat, Arc<str>, Details, Rc<RefCell<State>>), Error>
where
    S: Store,
{
    let bytes = tokio::time::timeout(Duration::from_secs(60), aggregate(stream))
        .await
        .map_err(|_| UploadError::AggregateTimeout)??;

    let permit = crate::process_semaphore().acquire().await?;

    let prescribed = Validations {
        image: &media.image,
        animation: &media.animation,
        video: &media.video,
    };

    tracing::trace!("Validating bytes");
    let (input_type, process_read) =
        crate::validate::validate_bytes(tmp_dir, bytes, prescribed, media.process_timeout).await?;

    let process_read = if let Some(operations) = media.preprocess_steps() {
        if let Some(format) = input_type.processable_format() {
            let (_, magick_args) =
                crate::processor::build_chain(operations, format.file_extension())?;

            let quality = match format {
                crate::formats::ProcessableFormat::Image(format) => media.image.quality_for(format),
                crate::formats::ProcessableFormat::Animation(format) => {
                    media.animation.quality_for(format)
                }
            };

            crate::magick::process_image_process_read(
                tmp_dir,
                process_read,
                magick_args,
                format,
                format,
                quality,
                media.process_timeout,
            )
            .await?
        } else {
            process_read
        }
    } else {
        process_read
    };

    let (state, identifier) = process_read
        .with_stdout(|stdout| async move {
            let hasher_reader = Hasher::new(stdout);
            let state = hasher_reader.state();

            store
                .save_async_read(hasher_reader, input_type.media_type())
                .await
                .map(move |identifier| (state, identifier))
        })
        .await??;

    let bytes_stream = store.to_bytes(&identifier, None, None).await?;
    let details =
        Details::from_bytes(tmp_dir, media.process_timeout, bytes_stream.into_bytes()).await?;

    drop(permit);

    Ok((input_type, identifier, details, state))
}

async fn dummy_ingest<S>(
    store: &S,
    stream: impl Stream<Item = Result<Bytes, Error>> + 'static,
) -> Result<(InternalFormat, Arc<str>, Details, Rc<RefCell<State>>), Error>
where
    S: Store,
{
    let stream = crate::stream::map(stream, |res| match res {
        Ok(bytes) => Ok(bytes),
        Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
    });

    let reader = Box::pin(tokio_util::io::StreamReader::new(stream));

    let hasher_reader = Hasher::new(reader);
    let state = hasher_reader.state();

    let input_type = InternalFormat::Image(crate::formats::ImageFormat::Png);

    let identifier = store
        .save_async_read(hasher_reader, input_type.media_type())
        .await?;

    let details = Details::danger_dummy(input_type);

    Ok((input_type, identifier, details, state))
}

#[tracing::instrument(skip(tmp_dir, repo, store, client, stream, config))]
pub(crate) async fn ingest<S>(
    tmp_dir: &TmpDir,
    repo: &ArcRepo,
    store: &S,
    client: &ClientWithMiddleware,
    stream: impl Stream<Item = Result<Bytes, Error>> + 'static,
    declared_alias: Option<Alias>,
    config: &crate::config::Configuration,
) -> Result<Session, Error>
where
    S: Store,
{
    let (input_type, identifier, details, state) = if config.server.danger_dummy_mode {
        dummy_ingest(store, stream).await?
    } else {
        process_ingest(tmp_dir, store, stream, &config.media).await?
    };

    let mut session = Session {
        repo: repo.clone(),
        delete_token: DeleteToken::generate(),
        hash: None,
        alias: None,
        identifier: Some(identifier.clone()),
    };

    if let Some(endpoint) = &config.media.external_validation {
        let stream = store.to_stream(&identifier, None, None).await?;

        let response = client
            .post(endpoint.as_str())
            .timeout(Duration::from_secs(
                config.media.external_validation_timeout,
            ))
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

    repo.relate_details(&identifier, &details).await?;

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

        metrics::counter!("pict-rs.ingest.end", "completed" => (!any_items).to_string())
            .increment(1);

        if self.hash.is_some() || self.alias.is_some() | self.identifier.is_some() {
            let cleanup_parent_span = tracing::info_span!(parent: None, "Dropped session cleanup");
            cleanup_parent_span.follows_from(Span::current());

            if let Some(hash) = self.hash.take() {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Session cleanup hash", hash = ?hash);

                crate::sync::spawn(
                    "session-cleanup-hash",
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
                    "session-cleanup-alias",
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
                    "session-cleanup-identifier",
                    async move {
                        let _ = crate::queue::cleanup_identifier(&repo, &identifier).await;
                    }
                    .instrument(cleanup_span),
                );
            }
        }
    }
}
