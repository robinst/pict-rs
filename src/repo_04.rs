use crate::{
    details::Details,
    repo::{Alias, DeleteToken},
};
use futures_core::Stream;
use std::{fmt::Debug, path::PathBuf, sync::Arc};

pub(crate) use self::sled::SledRepo;

mod sled;

#[tracing::instrument]
pub(crate) fn open(path: &PathBuf) -> color_eyre::Result<Option<SledRepo>> {
    SledRepo::build(path.clone())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RepoError {
    #[error("Error in sled")]
    SledError(#[from] self::sled::SledError),
}

pub(crate) trait BaseRepo {
    type Bytes: AsRef<[u8]> + From<Vec<u8>> + Clone;
}

impl<T> BaseRepo for actix_web::web::Data<T>
where
    T: BaseRepo,
{
    type Bytes = T::Bytes;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait SettingsRepo: BaseRepo {
    async fn get(&self, key: &'static str) -> Result<Option<Self::Bytes>, RepoError>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait IdentifierRepo: BaseRepo {
    async fn details(&self, identifier: Arc<str>) -> Result<Option<Details>, RepoError>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait HashRepo: BaseRepo {
    type Stream: Stream<Item = Result<Self::Bytes, RepoError>>;

    async fn size(&self) -> Result<u64, RepoError>;

    async fn hashes(&self) -> Self::Stream;

    async fn identifier(&self, hash: Self::Bytes) -> Result<Option<Arc<str>>, RepoError>;

    async fn variants(&self, hash: Self::Bytes) -> Result<Vec<(String, Arc<str>)>, RepoError>;

    async fn motion_identifier(&self, hash: Self::Bytes) -> Result<Option<Arc<str>>, RepoError>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait AliasRepo: BaseRepo {
    async fn delete_token(&self, alias: &Alias) -> Result<Option<DeleteToken>, RepoError>;

    async fn aliases_for_hash(&self, hash: Self::Bytes) -> Result<Vec<Alias>, RepoError>;
}
