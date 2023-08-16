use crate::{
    config,
    details::Details,
    repo::{Alias, DeleteToken},
    store::{Identifier, StoreError},
};
use futures_util::Stream;
use std::fmt::Debug;

pub(crate) use self::sled::SledRepo;

mod sled;

#[tracing::instrument]
pub(crate) fn open(config: &config::Sled) -> color_eyre::Result<Option<SledRepo>> {
    let config::Sled {
        path,
        cache_capacity,
        export_path: _,
    } = config;

    SledRepo::build(path.clone(), *cache_capacity)
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
    async fn details<I: Identifier>(&self, identifier: &I) -> Result<Option<Details>, StoreError>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait HashRepo: BaseRepo {
    type Stream: Stream<Item = Result<Self::Bytes, RepoError>>;

    async fn size(&self) -> Result<u64, RepoError>;

    async fn hashes(&self) -> Self::Stream;

    async fn identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, StoreError>;

    async fn variants<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Vec<(String, I)>, StoreError>;

    async fn motion_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, StoreError>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait AliasRepo: BaseRepo {
    async fn delete_token(&self, alias: &Alias) -> Result<Option<DeleteToken>, RepoError>;

    async fn for_hash(&self, hash: Self::Bytes) -> Result<Vec<Alias>, RepoError>;
}
