use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use crate::{
    bytes_stream::BytesStream,
    details::Details,
    error::{Error, UploadError},
    formats::InternalFormat,
    future::{WithMetrics, WithPollTimer},
    repo::{Alias, ArcRepo, DeleteToken, Hash},
    state::State,
    store::Store,
};
use actix_web::web::Bytes;
use futures_core::Stream;
use reqwest::Body;

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

async fn process_ingest<S>(
    state: &State<S>,
    stream: impl Stream<Item = Result<Bytes, Error>>,
) -> Result<
    (
        InternalFormat,
        Arc<str>,
        Details,
        Rc<RefCell<hasher::State>>,
    ),
    Error,
>
where
    S: Store,
{
    let bytes = tokio::time::timeout(
        Duration::from_secs(60),
        BytesStream::try_from_stream(stream),
    )
    .with_poll_timer("try-from-stream")
    .await
    .map_err(|_| UploadError::AggregateTimeout)??;

    let permit = crate::process_semaphore().acquire().await?;

    tracing::trace!("Validating bytes");
    let (input_type, process_read) = crate::validate::validate_bytes_stream(state, bytes)
        .with_poll_timer("validate-bytes-stream")
        .await?;

    let process_read = if let Some(operations) = state.config.media.preprocess_steps() {
        if let Some(format) = input_type.processable_format() {
            let (_, magick_args) =
                crate::processor::build_chain(operations, format.file_extension())?;

            let quality = match format {
                crate::formats::ProcessableFormat::Image(format) => {
                    state.config.media.image.quality_for(format)
                }
                crate::formats::ProcessableFormat::Animation(format) => {
                    state.config.media.animation.quality_for(format)
                }
            };

            let process =
                crate::magick::process_image_command(state, magick_args, format, format, quality)
                    .await?;

            process_read.pipe(process)
        } else {
            process_read
        }
    } else {
        process_read
    };

    let (hash_state, identifier) = process_read
        .with_stdout(|stdout| async move {
            let hasher_reader = Hasher::new(stdout);
            let hash_state = hasher_reader.state();

            state
                .store
                .save_stream(
                    tokio_util::io::ReaderStream::with_capacity(hasher_reader, 1024 * 64),
                    input_type.media_type(),
                    Some(input_type.file_extension()),
                )
                .with_poll_timer("save-hasher-reader")
                .await
                .map(move |identifier| (hash_state, identifier))
        })
        .with_poll_timer("save-process-stdout")
        .await??;

    let bytes_stream = state.store.to_bytes(&identifier, None, None).await?;
    let details = Details::from_bytes_stream(state, bytes_stream)
        .with_poll_timer("details-from-bytes-stream")
        .await?;

    drop(permit);

    Ok((input_type, identifier, details, hash_state))
}

async fn dummy_ingest<S>(
    state: &State<S>,
    stream: impl Stream<Item = Result<Bytes, Error>>,
) -> Result<
    (
        InternalFormat,
        Arc<str>,
        Details,
        Rc<RefCell<hasher::State>>,
    ),
    Error,
>
where
    S: Store,
{
    let stream = crate::stream::map(stream, |res| match res {
        Ok(bytes) => Ok(bytes),
        Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
    });

    let reader = tokio_util::io::StreamReader::new(stream);

    let hasher_reader = Hasher::new(reader);
    let hash_state = hasher_reader.state();

    let input_type = InternalFormat::Image(crate::formats::ImageFormat::Png);

    let identifier = state
        .store
        .save_stream(
            tokio_util::io::ReaderStream::with_capacity(hasher_reader, 1024 * 64),
            input_type.media_type(),
            Some(input_type.file_extension()),
        )
        .await?;

    let details = Details::danger_dummy(input_type);

    Ok((input_type, identifier, details, hash_state))
}

#[tracing::instrument(skip(state, stream))]
pub(crate) async fn ingest<S>(
    state: &State<S>,
    stream: impl Stream<Item = Result<Bytes, Error>>,
    declared_alias: Option<Alias>,
) -> Result<Session, Error>
where
    S: Store,
{
    let (input_type, identifier, details, hash_state) = if state.config.server.danger_dummy_mode {
        dummy_ingest(state, stream).await?
    } else {
        process_ingest(state, stream)
            .with_poll_timer("ingest-future")
            .await?
    };

    let mut session = Session {
        repo: state.repo.clone(),
        delete_token: DeleteToken::generate(),
        hash: None,
        alias: None,
        identifier: Some(identifier.clone()),
    };

    if let Some(endpoint) = &state.config.media.external_validation {
        let stream = state.store.to_stream(&identifier, None, None).await?;

        let response = state
            .client
            .post(endpoint.as_str())
            .timeout(Duration::from_secs(
                state.config.media.external_validation_timeout,
            ))
            .header("Content-Type", input_type.media_type().as_ref())
            .body(Body::wrap_stream(crate::stream::make_send(stream)))
            .send()
            .instrument(tracing::info_span!("external-validation"))
            .with_metrics(crate::init_metrics::INGEST_EXTERNAL_VALIDATION)
            .await?;

        if !response.status().is_success() {
            return Err(UploadError::FailedExternalValidation.into());
        }
    }

    let (hash, size) = hash_state.borrow_mut().finalize_reset();

    let hash = Hash::new(hash, size, input_type);

    save_upload(&mut session, state, hash.clone(), &identifier).await?;

    state.repo.relate_details(&identifier, &details).await?;

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
    state: &State<S>,
    hash: Hash,
    identifier: &Arc<str>,
) -> Result<(), Error>
where
    S: Store,
{
    if state
        .repo
        .create_hash(hash.clone(), identifier)
        .await?
        .is_err()
    {
        // duplicate upload
        state.store.remove(identifier).await?;
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
            tracing::trace!("create_alias: looping");

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
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let any_items = self.hash.is_some() || self.alias.is_some() || self.identifier.is_some();

        metrics::counter!(crate::init_metrics::INGEST_END, "completed" => (!any_items).to_string())
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
