use crate::{store::Identifier, upload_manager::Details};
use futures_util::Stream;
use uuid::Uuid;

pub(crate) struct Alias {
    id: Uuid,
    extension: String,
}
pub(crate) struct DeleteToken {
    id: Uuid,
}
pub(crate) struct AlreadyExists;

#[async_trait::async_trait]
pub(crate) trait SettingsRepo {
    type Error: std::error::Error;

    async fn set(&self, key: &'static [u8], value: Vec<u8>) -> Result<(), Self::Error>;
    async fn get(&self, key: &'static [u8]) -> Result<Option<Vec<u8>>, Self::Error>;
    async fn remove(&self, key: &'static [u8]) -> Result<(), Self::Error>;
}

#[async_trait::async_trait]
pub(crate) trait IdentifierRepo<I: Identifier> {
    type Hash: AsRef<[u8]>;
    type Error: std::error::Error;

    async fn relate_details(&self, identifier: I, details: Details) -> Result<(), Self::Error>;
    async fn details(&self, identifier: I) -> Result<Option<Details>, Self::Error>;

    async fn relate_hash(&self, identifier: I, hash: Self::Hash) -> Result<(), Self::Error>;
    async fn hash(&self, identifier: I) -> Result<Self::Hash, Self::Error>;

    async fn cleanup(&self, identifier: I) -> Result<(), Self::Error>;
}

#[async_trait::async_trait]
pub(crate) trait HashRepo {
    type Hash: AsRef<[u8]>;
    type Error: std::error::Error;
    type Stream: Stream<Item = Result<Self::Hash, Self::Error>>;

    async fn hashes(&self) -> Self::Stream;

    async fn create(&self, hash: Self::Hash) -> Result<Result<(), AlreadyExists>, Self::Error>;

    async fn relate_alias(&self, hash: Self::Hash, alias: Alias) -> Result<(), Self::Error>;
    async fn remove_alias(&self, hash: Self::Hash, alias: Alias) -> Result<(), Self::Error>;
    async fn aliases(&self, hash: Self::Hash) -> Result<Vec<Alias>, Self::Error>;

    async fn cleanup(&self, hash: Self::Hash) -> Result<(), Self::Error>;
}

#[async_trait::async_trait]
pub(crate) trait AliasRepo<I: Identifier> {
    type Hash: AsRef<[u8]>;
    type Error: std::error::Error;

    async fn create(&self, alias: Alias) -> Result<Result<(), AlreadyExists>, Self::Error>;

    async fn create_delete_token(
        &self,
        alias: Alias,
    ) -> Result<Result<DeleteToken, AlreadyExists>, Self::Error>;
    async fn delete_token(&self, alias: Alias) -> Result<DeleteToken, Self::Error>;

    async fn relate_hash(&self, alias: Alias, hash: Self::Hash) -> Result<(), Self::Error>;
    async fn hash(&self, alias: Alias) -> Result<Self::Hash, Self::Error>;

    async fn relate_identifier(&self, alias: Alias, identifier: I) -> Result<(), Self::Error>;
    async fn identifier(&self, alias: Alias) -> Result<I, Self::Error>;

    async fn cleanup(&self, alias: Alias) -> Result<(), Self::Error>;
}
