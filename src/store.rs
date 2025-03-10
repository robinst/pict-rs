use actix_web::web::Bytes;
use futures_core::Stream;
use std::{fmt::Debug, sync::Arc};

use crate::{bytes_stream::BytesStream, error_code::ErrorCode, stream::LocalBoxStream};

pub(crate) mod file_store;
pub(crate) mod object_store;

#[derive(Debug, thiserror::Error)]
pub(crate) enum StoreError {
    #[error("Error in file store")]
    FileStore(#[source] crate::store::file_store::FileError),

    #[error("Error in object store")]
    ObjectStore(#[source] crate::store::object_store::ObjectError),

    #[error("Error in DB")]
    Repo(#[from] crate::repo::RepoError),

    #[error("Error in 0.4 DB")]
    Repo04(#[from] crate::repo_04::RepoError),

    #[error("Error reading bytes stream")]
    ReadStream(#[source] std::io::Error),

    #[error("Requested file is not found")]
    FileNotFound(#[source] std::io::Error),

    #[error("Requested object is not found")]
    ObjectNotFound(#[source] crate::store::object_store::ObjectError),
}

impl StoreError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::FileStore(e) => e.error_code(),
            Self::ObjectStore(e) => e.error_code(),
            Self::Repo(e) => e.error_code(),
            Self::ReadStream(_) => ErrorCode::IO_ERROR,
            Self::Repo04(_) => ErrorCode::OLD_REPO_ERROR,
            Self::FileNotFound(_) | Self::ObjectNotFound(_) => ErrorCode::NOT_FOUND,
        }
    }

    pub(crate) const fn is_not_found(&self) -> bool {
        matches!(self, Self::FileNotFound(_)) || matches!(self, Self::ObjectNotFound(_))
    }

    pub(crate) const fn is_disconnected(&self) -> bool {
        match self {
            Self::Repo(e) => e.is_disconnected(),
            _ => false,
        }
    }
}

impl From<crate::store::file_store::FileError> for StoreError {
    fn from(value: crate::store::file_store::FileError) -> Self {
        match value {
            crate::store::file_store::FileError::Io(e)
                if e.kind() == std::io::ErrorKind::NotFound =>
            {
                Self::FileNotFound(e)
            }
            e => Self::FileStore(e),
        }
    }
}

impl From<crate::store::object_store::ObjectError> for StoreError {
    fn from(value: crate::store::object_store::ObjectError) -> Self {
        match value {
            e @ crate::store::object_store::ObjectError::Request(
                ::object_store::Error::NotFound { .. },
            ) => Self::ObjectNotFound(e),
            e => Self::ObjectStore(e),
        }
    }
}

pub(crate) trait Store: Clone + Debug {
    async fn health_check(&self) -> Result<(), StoreError>;

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
        extension: Option<&str>,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>>;

    fn public_url(&self, _: &Arc<str>) -> Option<url::Url>;

    async fn to_stream(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<LocalBoxStream<'static, std::io::Result<Bytes>>, StoreError>;

    async fn to_bytes(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<BytesStream, StoreError> {
        let stream = self.to_stream(identifier, from_start, len).await?;

        BytesStream::try_from_stream(stream)
            .await
            .map_err(StoreError::ReadStream)
    }

    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError>;

    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError>;
}

impl<T> Store for actix_web::web::Data<T>
where
    T: Store,
{
    async fn health_check(&self) -> Result<(), StoreError> {
        T::health_check(self).await
    }

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
        extension: Option<&str>,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>>,
    {
        T::save_stream(self, stream, content_type, extension).await
    }

    fn public_url(&self, identifier: &Arc<str>) -> Option<url::Url> {
        T::public_url(self, identifier)
    }

    async fn to_stream(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<LocalBoxStream<'static, std::io::Result<Bytes>>, StoreError> {
        T::to_stream(self, identifier, from_start, len).await
    }

    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError> {
        T::len(self, identifier).await
    }

    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError> {
        T::remove(self, identifier).await
    }
}

impl<T> Store for Arc<T>
where
    T: Store,
{
    async fn health_check(&self) -> Result<(), StoreError> {
        T::health_check(self).await
    }

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
        extension: Option<&str>,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>>,
    {
        T::save_stream(self, stream, content_type, extension).await
    }

    fn public_url(&self, identifier: &Arc<str>) -> Option<url::Url> {
        T::public_url(self, identifier)
    }

    async fn to_stream(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<LocalBoxStream<'static, std::io::Result<Bytes>>, StoreError> {
        T::to_stream(self, identifier, from_start, len).await
    }

    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError> {
        T::len(self, identifier).await
    }

    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError> {
        T::remove(self, identifier).await
    }
}

impl<'a, T> Store for &'a T
where
    T: Store,
{
    async fn health_check(&self) -> Result<(), StoreError> {
        T::health_check(self).await
    }

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
        extension: Option<&str>,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>>,
    {
        T::save_stream(self, stream, content_type, extension).await
    }

    fn public_url(&self, identifier: &Arc<str>) -> Option<url::Url> {
        T::public_url(self, identifier)
    }

    async fn to_stream(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<LocalBoxStream<'static, std::io::Result<Bytes>>, StoreError> {
        T::to_stream(self, identifier, from_start, len).await
    }

    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError> {
        T::len(self, identifier).await
    }

    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError> {
        T::remove(self, identifier).await
    }
}
