use crate::{details::Details, store::Identifier};
use futures_util::Stream;
use uuid::Uuid;

pub(crate) mod sled;

pub(crate) struct Alias {
    id: Uuid,
    extension: String,
}
pub(crate) struct DeleteToken {
    id: Uuid,
}
pub(crate) struct AlreadyExists;

impl Alias {
    fn to_bytes(&self) -> Vec<u8> {
        let mut v = self.id.as_bytes().to_vec();

        v.extend_from_slice(self.extension.as_bytes());

        v
    }

    fn from_slice(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 16 {
            let id = Uuid::from_slice(&bytes[0..16]).expect("Already checked length");
            let extension = String::from_utf8_lossy(&bytes[16..]).to_string();

            Some(Self { id, extension })
        } else {
            None
        }
    }
}

impl DeleteToken {
    fn to_bytes(&self) -> Vec<u8> {
        self.id.as_bytes().to_vec()
    }

    fn from_slice(bytes: &[u8]) -> Option<Self> {
        Some(DeleteToken {
            id: Uuid::from_slice(bytes).ok()?,
        })
    }
}

#[async_trait::async_trait]
pub(crate) trait SettingsRepo {
    type Bytes: AsRef<[u8]> + From<Vec<u8>>;
    type Error: std::error::Error;

    async fn set(&self, key: &'static [u8], value: Self::Bytes) -> Result<(), Self::Error>;
    async fn get(&self, key: &'static [u8]) -> Result<Option<Self::Bytes>, Self::Error>;
    async fn remove(&self, key: &'static [u8]) -> Result<(), Self::Error>;
}

#[async_trait::async_trait]
pub(crate) trait IdentifierRepo<I: Identifier> {
    type Bytes: AsRef<[u8]> + From<Vec<u8>>;
    type Error: std::error::Error;

    async fn relate_details(&self, identifier: I, details: Details) -> Result<(), Self::Error>;
    async fn details(&self, identifier: I) -> Result<Option<Details>, Self::Error>;

    async fn relate_hash(&self, identifier: I, hash: Self::Bytes) -> Result<(), Self::Error>;
    async fn hash(&self, identifier: I) -> Result<Self::Bytes, Self::Error>;

    async fn cleanup(&self, identifier: I) -> Result<(), Self::Error>;
}

#[async_trait::async_trait]
pub(crate) trait HashRepo<I: Identifier> {
    type Bytes: AsRef<[u8]> + From<Vec<u8>>;
    type Error: std::error::Error;
    type Stream: Stream<Item = Result<Self::Bytes, Self::Error>>;

    async fn hashes(&self) -> Self::Stream;

    async fn create(&self, hash: Self::Bytes) -> Result<Result<(), AlreadyExists>, Self::Error>;

    async fn relate_alias(&self, hash: Self::Bytes, alias: Alias) -> Result<(), Self::Error>;
    async fn remove_alias(&self, hash: Self::Bytes, alias: Alias) -> Result<(), Self::Error>;
    async fn aliases(&self, hash: Self::Bytes) -> Result<Vec<Alias>, Self::Error>;

    async fn relate_identifier(&self, hash: Self::Bytes, identifier: I) -> Result<(), Self::Error>;
    async fn identifier(&self, hash: Self::Bytes) -> Result<I, Self::Error>;

    async fn cleanup(&self, hash: Self::Bytes) -> Result<(), Self::Error>;
}

#[async_trait::async_trait]
pub(crate) trait AliasRepo {
    type Bytes: AsRef<[u8]> + From<Vec<u8>>;
    type Error: std::error::Error;

    async fn create(&self, alias: Alias) -> Result<Result<(), AlreadyExists>, Self::Error>;

    async fn relate_delete_token(
        &self,
        alias: Alias,
        delete_token: DeleteToken,
    ) -> Result<Result<(), AlreadyExists>, Self::Error>;
    async fn delete_token(&self, alias: Alias) -> Result<DeleteToken, Self::Error>;

    async fn relate_hash(&self, alias: Alias, hash: Self::Bytes) -> Result<(), Self::Error>;
    async fn hash(&self, alias: Alias) -> Result<Self::Bytes, Self::Error>;

    async fn cleanup(&self, alias: Alias) -> Result<(), Self::Error>;
}
