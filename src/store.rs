use std::fmt::Debug;

use actix_web::web::Bytes;
use futures_util::stream::Stream;
use tokio::io::{AsyncRead, AsyncWrite};

pub(crate) mod file_store;
pub(crate) mod object_store;

pub(crate) trait Identifier: Send + Sync + Clone + Debug {
    type Error: std::error::Error;

    fn to_bytes(&self) -> Result<Vec<u8>, Self::Error>;

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait Store: Send + Sync + Clone + Debug + 'static {
    type Error: std::error::Error;
    type Identifier: Identifier<Error = Self::Error>;
    type Stream: Stream<Item = std::io::Result<Bytes>>;

    async fn save_async_read<Reader>(
        &self,
        reader: &mut Reader,
        filename: &str,
    ) -> Result<Self::Identifier, Self::Error>
    where
        Reader: AsyncRead + Unpin;

    async fn save_bytes(
        &self,
        bytes: Bytes,
        filename: &str,
    ) -> Result<Self::Identifier, Self::Error>;

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, Self::Error>;

    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Send + Unpin;

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Self::Error>;

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Self::Error>;
}
