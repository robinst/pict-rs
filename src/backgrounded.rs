use crate::{
    error::Error,
    repo::{ArcRepo, UploadId},
    store::Store,
};
use actix_web::web::Bytes;
use futures_core::Stream;
use futures_util::TryStreamExt;
use mime::APPLICATION_OCTET_STREAM;
use tracing::{Instrument, Span};

pub(crate) struct Backgrounded<S>
where
    S: Store,
{
    repo: ArcRepo,
    identifier: Option<S::Identifier>,
    upload_id: Option<UploadId>,
}

impl<S> Backgrounded<S>
where
    S: Store,
{
    pub(crate) fn disarm(mut self) {
        let _ = self.identifier.take();
        let _ = self.upload_id.take();
    }

    pub(crate) fn upload_id(&self) -> Option<UploadId> {
        self.upload_id
    }

    pub(crate) fn identifier(&self) -> Option<&S::Identifier> {
        self.identifier.as_ref()
    }

    pub(crate) async fn proxy<P>(repo: ArcRepo, store: S, stream: P) -> Result<Self, Error>
    where
        P: Stream<Item = Result<Bytes, Error>> + Unpin + 'static,
    {
        let mut this = Self {
            repo,
            identifier: None,
            upload_id: Some(UploadId::generate()),
        };

        this.do_proxy(store, stream).await?;

        Ok(this)
    }

    async fn do_proxy<P>(&mut self, store: S, stream: P) -> Result<(), Error>
    where
        P: Stream<Item = Result<Bytes, Error>> + Unpin + 'static,
    {
        self.repo
            .create_upload(self.upload_id.expect("Upload id exists"))
            .await?;

        let stream = stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

        // use octet-stream, we don't know the upload's real type yet
        let identifier = store.save_stream(stream, APPLICATION_OCTET_STREAM).await?;

        self.identifier = Some(identifier);

        Ok(())
    }
}

impl<S> Drop for Backgrounded<S>
where
    S: Store,
{
    fn drop(&mut self) {
        let any_items = self.identifier.is_some() || self.upload_id.is_some();

        metrics::increment_counter!("pict-rs.background.upload", "completed" => (!any_items).to_string());

        if any_items {
            let cleanup_parent_span =
                tracing::info_span!(parent: None, "Dropped backgrounded cleanup");
            cleanup_parent_span.follows_from(Span::current());

            if let Some(identifier) = self.identifier.take() {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Backgrounded cleanup Identifier", identifier = ?identifier);

                tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
                    actix_rt::spawn(
                        async move {
                            let _ = crate::queue::cleanup_identifier(&repo, identifier).await;
                        }
                        .instrument(cleanup_span),
                    )
                });
            }

            if let Some(upload_id) = self.upload_id {
                let repo = self.repo.clone();

                let cleanup_span = tracing::info_span!(parent: &cleanup_parent_span, "Backgrounded cleanup Upload ID", upload_id = ?upload_id);

                tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
                    actix_rt::spawn(
                        async move {
                            let _ = repo.claim(upload_id).await;
                        }
                        .instrument(cleanup_span),
                    )
                });
            }
        }
    }
}
