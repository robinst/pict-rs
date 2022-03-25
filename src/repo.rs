use crate::config::RequiredSledRepo;
use crate::{config::Repository, details::Details, store::Identifier};
use futures_util::Stream;
use uuid::Uuid;

mod old;
pub(crate) mod sled;

#[derive(Debug)]
pub(crate) enum Repo {
    Sled(self::sled::SledRepo),
}

#[derive(Clone, Debug)]
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
    type Bytes: AsRef<[u8]> + From<Vec<u8>>;
    type Error: std::error::Error;

    async fn set(&self, key: &'static [u8], value: Self::Bytes) -> Result<(), Self::Error>;
    async fn get(&self, key: &'static [u8]) -> Result<Option<Self::Bytes>, Self::Error>;
    async fn remove(&self, key: &'static [u8]) -> Result<(), Self::Error>;
}

#[async_trait::async_trait]
pub(crate) trait IdentifierRepo<I: Identifier> {
    type Error: std::error::Error;

    async fn relate_details(&self, identifier: I, details: Details) -> Result<(), Self::Error>;
    async fn details(&self, identifier: I) -> Result<Option<Details>, Self::Error>;

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

    async fn relate_variant_identifier(
        &self,
        hash: Self::Bytes,
        variant: String,
        identifier: I,
    ) -> Result<(), Self::Error>;
    async fn variant_identifier(
        &self,
        hash: Self::Bytes,
        variant: String,
    ) -> Result<Option<I>, Self::Error>;

    async fn relate_motion_identifier(
        &self,
        hash: Self::Bytes,
        identifier: I,
    ) -> Result<(), Self::Error>;
    async fn motion_identifier(&self, hash: Self::Bytes) -> Result<Option<I>, Self::Error>;

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

impl Repo {
    pub(crate) fn open(config: Repository) -> anyhow::Result<Self> {
        match config {
            Repository::Sled(RequiredSledRepo {
                mut path,
                cache_capacity,
            }) => {
                path.push("v0.4.0-alpha.1");

                let db = ::sled::Config::new()
                    .cache_capacity(cache_capacity)
                    .path(path)
                    .open()?;

                Ok(Self::Sled(self::sled::SledRepo::new(db)?))
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub(crate) async fn from_db(&self, db: ::sled::Db) -> anyhow::Result<()> {
        if self.has_migrated().await? {
            return Ok(());
        }

        let old = self::old::Old::open(db)?;

        for hash in old.hashes() {
            match self {
                Self::Sled(repo) => {
                    if let Err(e) = migrate_hash(repo, &old, hash).await {
                        tracing::error!("Failed to migrate hash: {}", e);
                    }
                }
            }
        }

        self.mark_migrated().await?;

        Ok(())
    }

    async fn has_migrated(&self) -> anyhow::Result<bool> {
        match self {
            Self::Sled(repo) => Ok(repo.get(REPO_MIGRATION_O1).await?.is_some()),
        }
    }

    async fn mark_migrated(&self) -> anyhow::Result<()> {
        match self {
            Self::Sled(repo) => {
                repo.set(REPO_MIGRATION_O1, b"1".to_vec().into()).await?;
            }
        }

        Ok(())
    }
}

const REPO_MIGRATION_O1: &[u8] = b"repo-migration-01";
const STORE_MIGRATION_PROGRESS: &[u8] = b"store-migration-progress";
const GENERATOR_KEY: &[u8] = b"last-path";

async fn migrate_hash<T>(repo: &T, old: &old::Old, hash: ::sled::IVec) -> anyhow::Result<()>
where
    T: IdentifierRepo<::sled::IVec>,
    <T as IdentifierRepo<::sled::IVec>>::Error: Send + Sync + 'static,
    T: HashRepo<::sled::IVec>,
    <T as HashRepo<::sled::IVec>>::Error: Send + Sync + 'static,
    T: AliasRepo,
    <T as AliasRepo>::Error: Send + Sync + 'static,
    T: SettingsRepo,
    <T as SettingsRepo>::Error: Send + Sync + 'static,
{
    HashRepo::create(repo, hash.to_vec().into()).await?;

    let main_ident = old.main_identifier(&hash)?;

    HashRepo::relate_identifier(repo, hash.to_vec().into(), main_ident.clone()).await?;

    for alias in old.aliases(&hash) {
        if let Ok(Ok(())) = AliasRepo::create(repo, alias.clone()).await {
            let _ = HashRepo::relate_alias(repo, hash.to_vec().into(), alias.clone()).await;
            let _ = AliasRepo::relate_hash(repo, alias.clone(), hash.to_vec().into()).await;

            if let Ok(Some(delete_token)) = old.delete_token(&alias) {
                let _ = AliasRepo::relate_delete_token(repo, alias, delete_token).await;
            }
        }
    }

    if let Ok(Some(identifier)) = old.motion_identifier(&hash) {
        HashRepo::relate_motion_identifier(repo, hash.to_vec().into(), identifier).await;
    }

    for (variant, identifier) in old.variants(&hash)? {
        let _ =
            HashRepo::relate_variant_identifier(repo, hash.to_vec().into(), variant, identifier)
                .await;
    }

    for (identifier, details) in old.details(&hash)? {
        let _ = IdentifierRepo::relate_details(repo, identifier, details).await;
    }

    if let Ok(Some(value)) = old.setting(STORE_MIGRATION_PROGRESS) {
        SettingsRepo::set(repo, STORE_MIGRATION_PROGRESS, value.to_vec().into()).await?;
    }

    if let Ok(Some(value)) = old.setting(GENERATOR_KEY) {
        SettingsRepo::set(repo, GENERATOR_KEY, value.to_vec().into()).await?;
    }

    Ok(())
}

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

impl std::fmt::Display for Alias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.id, self.extension)
    }
}

impl Identifier for Vec<u8> {
    type Error = std::convert::Infallible;

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Ok(bytes)
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.clone())
    }
}

impl Identifier for ::sled::IVec {
    type Error = std::convert::Infallible;

    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Ok(bytes.into())
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.to_vec())
    }
}
