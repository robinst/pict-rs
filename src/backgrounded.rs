use crate::{
    error::Error,
    repo::{FullRepo, UploadId, UploadRepo},
    store::Store,
};
use actix_web::web::Bytes;
use futures_util::{Stream, TryStreamExt};
use tokio_util::io::StreamReader;
use tracing::{Instrument, Span};

pub(crate) struct Backgrounded<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    repo: R,
    identifier: Option<S::Identifier>,
    upload_id: Option<UploadId>,
}

impl<R, S> Backgrounded<R, S>
where
    R: FullRepo + 'static,
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

    pub(crate) async fn proxy<P>(repo: R, store: S, stream: P) -> Result<Self, Error>
    where
        P: Stream<Item = Result<Bytes, Error>>,
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
        P: Stream<Item = Result<Bytes, Error>>,
    {
        UploadRepo::create(&self.repo, self.upload_id.expect("Upload id exists")).await?;

        let stream = stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
        let mut reader = StreamReader::new(Box::pin(stream));

        let identifier = store.save_async_read(&mut reader).await?;

        self.identifier = Some(identifier);

        Ok(())
    }
}

impl<R, S> Drop for Backgrounded<R, S>
where
    R: FullRepo + 'static,
    S: Store,
{
    #[tracing::instrument(name = "Drop Backgrounded", skip(self), fields(identifier = ?self.identifier, upload_id = ?self.upload_id))]
    fn drop(&mut self) {
        if let Some(identifier) = self.identifier.take() {
            let repo = self.repo.clone();

            let cleanup_span = tracing::info_span!(parent: None, "Backgrounded cleanup Identifier", identifier = ?identifier);
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

        if let Some(upload_id) = self.upload_id {
            let repo = self.repo.clone();

            let cleanup_span = tracing::info_span!(parent: None, "Backgrounded cleanup Upload ID", upload_id = ?upload_id);
            cleanup_span.follows_from(Span::current());

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
