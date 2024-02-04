use std::sync::Arc;

use crate::{
    error::Error,
    repo::{ArcRepo, UploadId},
    state::State,
    store::Store,
};
use actix_web::web::Bytes;
use futures_core::Stream;
use mime::APPLICATION_OCTET_STREAM;
use tracing::{Instrument, Span};

pub(crate) struct Backgrounded {
    repo: ArcRepo,
    identifier: Option<Arc<str>>,
    upload_id: Option<UploadId>,
}

impl Backgrounded {
    pub(crate) fn disarm(mut self) {
        let _ = self.identifier.take();
        let _ = self.upload_id.take();
    }

    pub(crate) fn upload_id(&self) -> Option<UploadId> {
        self.upload_id
    }

    pub(crate) fn identifier(&self) -> Option<&Arc<str>> {
        self.identifier.as_ref()
    }

    pub(crate) async fn proxy<S, P>(state: &State<S>, stream: P) -> Result<Self, Error>
    where
        S: Store,
        P: Stream<Item = Result<Bytes, Error>> + 'static,
    {
        let mut this = Self {
            repo: state.repo.clone(),
            identifier: None,
            upload_id: None,
        };

        this.do_proxy(&state.store, stream).await?;

        Ok(this)
    }

    async fn do_proxy<S, P>(&mut self, store: &S, stream: P) -> Result<(), Error>
    where
        S: Store,
        P: Stream<Item = Result<Bytes, Error>> + 'static,
    {
        self.upload_id = Some(self.repo.create_upload().await?);

        let stream = Box::pin(crate::stream::map_err(stream, |e| {
            std::io::Error::new(std::io::ErrorKind::Other, e)
        }));

        // use octet-stream, we don't know the upload's real type yet
        let identifier = store.save_stream(stream, APPLICATION_OCTET_STREAM).await?;

        self.identifier = Some(identifier);

        Ok(())
    }
}

impl Drop for Backgrounded {
    fn drop(&mut self) {
        let any_items = self.identifier.is_some() || self.upload_id.is_some();

        metrics::counter!("pict-rs.background.upload", "completed" => (!any_items).to_string())
            .increment(1);

        if any_items {
            let cleanup_parent_span =
                tracing::info_span!(parent: None, "Dropped backgrounded cleanup");
            cleanup_parent_span.follows_from(Span::current());

            if let Some(identifier) = self.identifier.take() {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Backgrounded cleanup Identifier", identifier = ?identifier);

                crate::sync::spawn(
                    "backgrounded-cleanup-identifier",
                    async move {
                        let _ = crate::queue::cleanup_identifier(&repo, &identifier).await;
                    }
                    .instrument(cleanup_span),
                );
            }

            if let Some(upload_id) = self.upload_id {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Backgrounded cleanup Upload ID", upload_id = ?upload_id);

                crate::sync::spawn(
                    "backgrounded-claim-upload",
                    async move {
                        let _ = repo.claim(upload_id).await;
                    }
                    .instrument(cleanup_span),
                );
            }
        }
    }
}
