use actix_web::web::Bytes;
use base64::{prelude::BASE64_STANDARD, Engine};
use futures_core::Stream;
use std::{fmt::Debug, sync::Arc};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error_code::ErrorCode;

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
            Self::Repo04(_) => ErrorCode::OLD_REPO_ERROR,
            Self::FileNotFound(_) | Self::ObjectNotFound(_) => ErrorCode::NOT_FOUND,
        }
    }
    pub(crate) const fn is_not_found(&self) -> bool {
        matches!(self, Self::FileNotFound(_)) || matches!(self, Self::ObjectNotFound(_))
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
            e @ crate::store::object_store::ObjectError::Status(
                actix_web::http::StatusCode::NOT_FOUND,
                _,
            ) => Self::ObjectNotFound(e),
            e => Self::ObjectStore(e),
        }
    }
}

pub(crate) trait Identifier: Send + Sync + Debug {
    fn to_bytes(&self) -> Result<Vec<u8>, StoreError>;

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, StoreError>
    where
        Self: Sized;

    fn from_arc(arc: Arc<[u8]>) -> Result<Self, StoreError>
    where
        Self: Sized;

    fn string_repr(&self) -> String;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait Store: Clone + Debug {
    type Identifier: Identifier + Clone + 'static;
    type Stream: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static;

    async fn health_check(&self) -> Result<(), StoreError>;

    async fn save_async_read<Reader>(
        &self,
        reader: Reader,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static;

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static;

    async fn save_bytes(
        &self,
        bytes: Bytes,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError>;

    fn public_url(&self, _: &Self::Identifier) -> Option<url::Url>;

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, StoreError>;

    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Unpin;

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, StoreError>;

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), StoreError>;
}

#[async_trait::async_trait(?Send)]
impl<T> Store for actix_web::web::Data<T>
where
    T: Store,
{
    type Identifier = T::Identifier;
    type Stream = T::Stream;

    async fn health_check(&self) -> Result<(), StoreError> {
        T::health_check(self).await
    }

    async fn save_async_read<Reader>(
        &self,
        reader: Reader,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        T::save_async_read(self, reader, content_type).await
    }

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        T::save_stream(self, stream, content_type).await
    }

    async fn save_bytes(
        &self,
        bytes: Bytes,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError> {
        T::save_bytes(self, bytes, content_type).await
    }

    fn public_url(&self, identifier: &Self::Identifier) -> Option<url::Url> {
        T::public_url(self, identifier)
    }

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, StoreError> {
        T::to_stream(self, identifier, from_start, len).await
    }

    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Unpin,
    {
        T::read_into(self, identifier, writer).await
    }

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, StoreError> {
        T::len(self, identifier).await
    }

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), StoreError> {
        T::remove(self, identifier).await
    }
}

#[async_trait::async_trait(?Send)]
impl<'a, T> Store for &'a T
where
    T: Store,
{
    type Identifier = T::Identifier;
    type Stream = T::Stream;

    async fn health_check(&self) -> Result<(), StoreError> {
        T::health_check(self).await
    }

    async fn save_async_read<Reader>(
        &self,
        reader: Reader,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        T::save_async_read(self, reader, content_type).await
    }

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        T::save_stream(self, stream, content_type).await
    }

    async fn save_bytes(
        &self,
        bytes: Bytes,
        content_type: mime::Mime,
    ) -> Result<Self::Identifier, StoreError> {
        T::save_bytes(self, bytes, content_type).await
    }

    fn public_url(&self, identifier: &Self::Identifier) -> Option<url::Url> {
        T::public_url(self, identifier)
    }

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, StoreError> {
        T::to_stream(self, identifier, from_start, len).await
    }

    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Unpin,
    {
        T::read_into(self, identifier, writer).await
    }

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, StoreError> {
        T::len(self, identifier).await
    }

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), StoreError> {
        T::remove(self, identifier).await
    }
}

impl Identifier for Vec<u8> {
    fn from_bytes(bytes: Vec<u8>) -> Result<Self, StoreError>
    where
        Self: Sized,
    {
        Ok(bytes)
    }

    fn from_arc(arc: Arc<[u8]>) -> Result<Self, StoreError>
    where
        Self: Sized,
    {
        Ok(Vec::from(&arc[..]))
    }

    fn to_bytes(&self) -> Result<Vec<u8>, StoreError> {
        Ok(self.clone())
    }

    fn string_repr(&self) -> String {
        BASE64_STANDARD.encode(self.as_slice())
    }
}

impl Identifier for Arc<[u8]> {
    fn from_bytes(bytes: Vec<u8>) -> Result<Self, StoreError>
    where
        Self: Sized,
    {
        Ok(Arc::from(bytes))
    }

    fn from_arc(arc: Arc<[u8]>) -> Result<Self, StoreError>
    where
        Self: Sized,
    {
        Ok(arc)
    }

    fn to_bytes(&self) -> Result<Vec<u8>, StoreError> {
        Ok(Vec::from(&self[..]))
    }

    fn string_repr(&self) -> String {
        BASE64_STANDARD.encode(&self[..])
    }
}
