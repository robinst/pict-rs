use actix_web::web::Bytes;
use futures_util::stream::Stream;
use std::fmt::Debug;
use tokio::io::{AsyncRead, AsyncWrite};

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

    #[error("Requested file is not found")]
    FileNotFound(#[source] std::io::Error),

    #[error("Requested object is not found")]
    ObjectNotFound(#[source] crate::store::object_store::ObjectError),
}

impl StoreError {
    pub(crate) const fn is_not_found(&self) -> bool {
        matches!(self, Self::FileNotFound(_)) || matches!(self, Self::ObjectNotFound(_))
    }

    pub(crate) const fn is_missing(&self) -> bool {
        match self {
            Self::Repo(e) => e.is_missing(),
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
            e @ crate::store::object_store::ObjectError::Status(
                actix_web::http::StatusCode::NOT_FOUND,
                _,
            ) => Self::ObjectNotFound(e),
            e => Self::ObjectStore(e),
        }
    }
}

pub(crate) trait Identifier: Send + Sync + Clone + Debug {
    fn to_bytes(&self) -> Result<Vec<u8>, StoreError>;

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, StoreError>
    where
        Self: Sized;

    fn string_repr(&self) -> String;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait Store: Clone + Debug {
    type Identifier: Identifier + 'static;
    type Stream: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static;

    async fn health_check(&self) -> Result<(), StoreError>;

    async fn save_async_read<Reader>(&self, reader: Reader) -> Result<Self::Identifier, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static;

    async fn save_stream<S>(&self, stream: S) -> Result<Self::Identifier, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static;

    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, StoreError>;

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

    async fn save_async_read<Reader>(&self, reader: Reader) -> Result<Self::Identifier, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        T::save_async_read(self, reader).await
    }

    async fn save_stream<S>(&self, stream: S) -> Result<Self::Identifier, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        T::save_stream(self, stream).await
    }

    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, StoreError> {
        T::save_bytes(self, bytes).await
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

    async fn save_async_read<Reader>(&self, reader: Reader) -> Result<Self::Identifier, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        T::save_async_read(self, reader).await
    }

    async fn save_stream<S>(&self, stream: S) -> Result<Self::Identifier, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        T::save_stream(self, stream).await
    }

    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, StoreError> {
        T::save_bytes(self, bytes).await
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
