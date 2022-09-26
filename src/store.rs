use crate::error::Error;
use actix_web::web::Bytes;
use futures_util::stream::Stream;
use std::fmt::Debug;
use tokio::io::{AsyncRead, AsyncWrite};

pub(crate) mod file_store;
pub(crate) mod object_store;

pub(crate) trait Identifier: Send + Sync + Clone + Debug {
    fn to_bytes(&self) -> Result<Vec<u8>, Error>;

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Error>
    where
        Self: Sized;

    fn string_repr(&self) -> String;
}

pub(crate) trait StoreConfig: Send + Sync + Clone {
    type Store: Store;

    fn build(self) -> Self::Store;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait Store: Clone + Debug {
    type Identifier: Identifier + 'static;
    type Stream: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static;

    async fn save_async_read<Reader>(&self, reader: Reader) -> Result<Self::Identifier, Error>
    where
        Reader: AsyncRead + Unpin + 'static;

    async fn save_stream<S>(&self, stream: S) -> Result<Self::Identifier, Error>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static;

    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, Error>;

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, Error>;

    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Unpin;

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Error>;

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Error>;
}

#[async_trait::async_trait(?Send)]
impl<T> Store for actix_web::web::Data<T>
where
    T: Store,
{
    type Identifier = T::Identifier;
    type Stream = T::Stream;

    async fn save_async_read<Reader>(&self, reader: Reader) -> Result<Self::Identifier, Error>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        T::save_async_read(self, reader).await
    }

    async fn save_stream<S>(&self, stream: S) -> Result<Self::Identifier, Error>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        T::save_stream(self, stream).await
    }

    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, Error> {
        T::save_bytes(self, bytes).await
    }

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, Error> {
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

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Error> {
        T::len(self, identifier).await
    }

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Error> {
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

    async fn save_async_read<Reader>(&self, reader: Reader) -> Result<Self::Identifier, Error>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        T::save_async_read(self, reader).await
    }

    async fn save_stream<S>(&self, stream: S) -> Result<Self::Identifier, Error>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        T::save_stream(self, stream).await
    }

    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, Error> {
        T::save_bytes(self, bytes).await
    }

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, Error> {
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

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Error> {
        T::len(self, identifier).await
    }

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Error> {
        T::remove(self, identifier).await
    }
}
