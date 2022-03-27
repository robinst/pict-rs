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
}

#[async_trait::async_trait(?Send)]
pub(crate) trait Store: Send + Sync + Clone + Debug + 'static {
    type Identifier: Identifier;
    type Stream: Stream<Item = std::io::Result<Bytes>>;

    async fn save_async_read<Reader>(&self, reader: &mut Reader) -> Result<Self::Identifier, Error>
    where
        Reader: AsyncRead + Unpin;

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
        Writer: AsyncWrite + Send + Unpin;

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Error>;

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Error>;
}
