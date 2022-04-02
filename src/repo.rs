use crate::{config, details::Details, error::Error, store::Identifier};
use futures_util::Stream;
use std::fmt::Debug;
use tracing::debug;
use uuid::Uuid;

mod old;
pub(crate) mod sled;

#[derive(Clone, Debug)]
pub(crate) enum Repo {
    Sled(self::sled::SledRepo),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum MaybeUuid {
    Uuid(Uuid),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Alias {
    id: MaybeUuid,
    extension: Option<String>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DeleteToken {
    id: MaybeUuid,
}

pub(crate) struct AlreadyExists;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UploadId {
    id: Uuid,
}

pub(crate) enum UploadResult {
    Success { alias: Alias, token: DeleteToken },
    Failure { message: String },
}

#[async_trait::async_trait(?Send)]
pub(crate) trait FullRepo:
    UploadRepo
    + SettingsRepo
    + IdentifierRepo
    + AliasRepo
    + QueueRepo
    + HashRepo
    + Send
    + Sync
    + Clone
    + Debug
{
    async fn identifier_from_alias<I: Identifier + 'static>(
        &self,
        alias: &Alias,
    ) -> Result<I, Error> {
        let hash = self.hash(alias).await?;
        self.identifier(hash).await
    }

    async fn aliases_from_alias(&self, alias: &Alias) -> Result<Vec<Alias>, Error> {
        let hash = self.hash(alias).await?;
        self.aliases(hash).await
    }

    async fn still_identifier_from_alias<I: Identifier + 'static>(
        &self,
        alias: &Alias,
    ) -> Result<Option<I>, Error> {
        let hash = self.hash(alias).await?;
        let identifier = self.identifier::<I>(hash.clone()).await?;

        match self.details(&identifier).await? {
            Some(details) if details.is_motion() => self.motion_identifier::<I>(hash).await,
            Some(_) => Ok(Some(identifier)),
            None => Ok(None),
        }
    }
}

pub(crate) trait BaseRepo {
    type Bytes: AsRef<[u8]> + From<Vec<u8>> + Clone;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait UploadRepo: BaseRepo {
    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, Error>;

    async fn claim(&self, upload_id: UploadId) -> Result<(), Error>;

    async fn complete(&self, upload_id: UploadId, result: UploadResult) -> Result<(), Error>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait QueueRepo: BaseRepo {
    async fn in_progress(&self, worker_id: Vec<u8>) -> Result<Option<Self::Bytes>, Error>;

    async fn push(&self, queue: &'static str, job: Self::Bytes) -> Result<(), Error>;

    async fn pop(&self, queue: &'static str, worker_id: Vec<u8>) -> Result<Self::Bytes, Error>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait SettingsRepo: BaseRepo {
    async fn set(&self, key: &'static str, value: Self::Bytes) -> Result<(), Error>;
    async fn get(&self, key: &'static str) -> Result<Option<Self::Bytes>, Error>;
    async fn remove(&self, key: &'static str) -> Result<(), Error>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait IdentifierRepo: BaseRepo {
    async fn relate_details<I: Identifier>(
        &self,
        identifier: &I,
        details: &Details,
    ) -> Result<(), Error>;
    async fn details<I: Identifier>(&self, identifier: &I) -> Result<Option<Details>, Error>;

    async fn cleanup<I: Identifier>(&self, identifier: &I) -> Result<(), Error>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait HashRepo: BaseRepo {
    type Stream: Stream<Item = Result<Self::Bytes, Error>>;

    async fn hashes(&self) -> Self::Stream;

    async fn create(&self, hash: Self::Bytes) -> Result<Result<(), AlreadyExists>, Error>;

    async fn relate_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Error>;
    async fn remove_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Error>;
    async fn aliases(&self, hash: Self::Bytes) -> Result<Vec<Alias>, Error>;

    async fn relate_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Error>;
    async fn identifier<I: Identifier + 'static>(&self, hash: Self::Bytes) -> Result<I, Error>;

    async fn relate_variant_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        variant: String,
        identifier: &I,
    ) -> Result<(), Error>;
    async fn variant_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
        variant: String,
    ) -> Result<Option<I>, Error>;
    async fn variants<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Vec<(String, I)>, Error>;

    async fn relate_motion_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Error>;
    async fn motion_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, Error>;

    async fn cleanup(&self, hash: Self::Bytes) -> Result<(), Error>;
}

#[async_trait::async_trait(?Send)]
pub(crate) trait AliasRepo: BaseRepo {
    async fn create(&self, alias: &Alias) -> Result<Result<(), AlreadyExists>, Error>;

    async fn relate_delete_token(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
    ) -> Result<Result<(), AlreadyExists>, Error>;
    async fn delete_token(&self, alias: &Alias) -> Result<DeleteToken, Error>;

    async fn relate_hash(&self, alias: &Alias, hash: Self::Bytes) -> Result<(), Error>;
    async fn hash(&self, alias: &Alias) -> Result<Self::Bytes, Error>;

    async fn cleanup(&self, alias: &Alias) -> Result<(), Error>;
}

impl Repo {
    pub(crate) fn open(config: config::Repo) -> color_eyre::Result<Self> {
        match config {
            config::Repo::Sled(config::Sled {
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
    pub(crate) async fn from_db(&self, db: ::sled::Db) -> color_eyre::Result<()> {
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

    async fn has_migrated(&self) -> color_eyre::Result<bool> {
        match self {
            Self::Sled(repo) => Ok(repo.get(REPO_MIGRATION_O1).await?.is_some()),
        }
    }

    async fn mark_migrated(&self) -> color_eyre::Result<()> {
        match self {
            Self::Sled(repo) => {
                repo.set(REPO_MIGRATION_O1, b"1".to_vec().into()).await?;
            }
        }

        Ok(())
    }
}

const REPO_MIGRATION_O1: &str = "repo-migration-01";
const STORE_MIGRATION_PROGRESS: &str = "store-migration-progress";
const GENERATOR_KEY: &str = "last-path";

async fn migrate_hash<T>(repo: &T, old: &old::Old, hash: ::sled::IVec) -> color_eyre::Result<()>
where
    T: IdentifierRepo + HashRepo + AliasRepo + SettingsRepo,
{
    if HashRepo::create(repo, hash.to_vec().into()).await?.is_err() {
        debug!("Duplicate hash detected");
        return Ok(());
    }

    let main_ident = old.main_identifier(&hash)?.to_vec();

    repo.relate_identifier(hash.to_vec().into(), &main_ident)
        .await?;

    for alias in old.aliases(&hash) {
        if let Ok(Ok(())) = AliasRepo::create(repo, &alias).await {
            let _ = repo.relate_alias(hash.to_vec().into(), &alias).await;
            let _ = repo.relate_hash(&alias, hash.to_vec().into()).await;

            if let Ok(Some(delete_token)) = old.delete_token(&alias) {
                let _ = repo.relate_delete_token(&alias, &delete_token).await;
            }
        }
    }

    if let Ok(Some(identifier)) = old.motion_identifier(&hash) {
        let _ = repo
            .relate_motion_identifier(hash.to_vec().into(), &identifier.to_vec())
            .await;
    }

    for (variant_path, identifier) in old.variants(&hash)? {
        let variant = variant_path.to_string_lossy().to_string();

        let _ = repo
            .relate_variant_identifier(hash.to_vec().into(), variant, &identifier.to_vec())
            .await;
    }

    for (identifier, details) in old.details(&hash)? {
        let _ = repo.relate_details(&identifier.to_vec(), &details).await;
    }

    if let Ok(Some(value)) = old.setting(STORE_MIGRATION_PROGRESS.as_bytes()) {
        repo.set(STORE_MIGRATION_PROGRESS, value.to_vec().into())
            .await?;
    }

    if let Ok(Some(value)) = old.setting(GENERATOR_KEY.as_bytes()) {
        repo.set(GENERATOR_KEY, value.to_vec().into()).await?;
    }

    Ok(())
}

impl MaybeUuid {
    fn from_str(s: &str) -> Self {
        if let Ok(uuid) = Uuid::parse_str(s) {
            MaybeUuid::Uuid(uuid)
        } else {
            MaybeUuid::Name(s.into())
        }
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Uuid(uuid) => &uuid.as_bytes()[..],
            Self::Name(name) => name.as_bytes(),
        }
    }
}

fn split_at_dot(s: &str) -> Option<(&str, &str)> {
    let index = s.find('.')?;

    Some(s.split_at(index))
}

impl Alias {
    pub(crate) fn generate(extension: String) -> Self {
        Alias {
            id: MaybeUuid::Uuid(Uuid::new_v4()),
            extension: Some(extension),
        }
    }

    pub(crate) fn from_existing(alias: &str) -> Self {
        if let Some((start, end)) = split_at_dot(alias) {
            Alias {
                id: MaybeUuid::from_str(start),
                extension: Some(end.into()),
            }
        } else {
            Alias {
                id: MaybeUuid::from_str(alias),
                extension: None,
            }
        }
    }

    pub(crate) fn extension(&self) -> Option<&str> {
        self.extension.as_deref()
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut v = self.id.as_bytes().to_vec();

        if let Some(ext) = self.extension() {
            v.extend_from_slice(ext.as_bytes());
        }

        v
    }

    fn from_slice(bytes: &[u8]) -> Option<Self> {
        if let Ok(s) = std::str::from_utf8(bytes) {
            Some(Self::from_existing(s))
        } else if bytes.len() >= 16 {
            let id = Uuid::from_slice(&bytes[0..16]).expect("Already checked length");

            let extension = if bytes.len() > 16 {
                Some(String::from_utf8_lossy(&bytes[16..]).to_string())
            } else {
                None
            };

            Some(Self {
                id: MaybeUuid::Uuid(id),
                extension,
            })
        } else {
            None
        }
    }
}

impl DeleteToken {
    pub(crate) fn from_existing(existing: &str) -> Self {
        if let Ok(uuid) = Uuid::parse_str(existing) {
            DeleteToken {
                id: MaybeUuid::Uuid(uuid),
            }
        } else {
            DeleteToken {
                id: MaybeUuid::Name(existing.into()),
            }
        }
    }

    pub(crate) fn generate() -> Self {
        Self {
            id: MaybeUuid::Uuid(Uuid::new_v4()),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        self.id.as_bytes().to_vec()
    }

    fn from_slice(bytes: &[u8]) -> Option<Self> {
        if let Ok(s) = std::str::from_utf8(bytes) {
            Some(DeleteToken::from_existing(s))
        } else if bytes.len() == 16 {
            Some(DeleteToken {
                id: MaybeUuid::Uuid(Uuid::from_slice(bytes).ok()?),
            })
        } else {
            None
        }
    }
}

impl UploadId {
    pub(crate) fn generate() -> Self {
        Self { id: Uuid::new_v4() }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.id.as_bytes()[..]
    }

    pub(crate) fn from_bytes(&self, bytes: &[u8]) -> Option<Self> {
        let id = Uuid::from_slice(bytes).ok()?;
        Some(Self { id })
    }
}

impl From<Uuid> for UploadId {
    fn from(id: Uuid) -> Self {
        Self { id }
    }
}

impl std::fmt::Display for MaybeUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uuid(id) => write!(f, "{}", id),
            Self::Name(name) => write!(f, "{}", name),
        }
    }
}

impl std::str::FromStr for DeleteToken {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(DeleteToken::from_existing(s))
    }
}

impl std::fmt::Display for DeleteToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl std::str::FromStr for Alias {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Alias::from_existing(s))
    }
}

impl std::fmt::Display for Alias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ext) = self.extension() {
            write!(f, "{}{}", self.id, ext)
        } else {
            write!(f, "{}", self.id)
        }
    }
}

impl Identifier for Vec<u8> {
    fn from_bytes(bytes: Vec<u8>) -> Result<Self, Error>
    where
        Self: Sized,
    {
        Ok(bytes)
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        Ok(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{Alias, DeleteToken, MaybeUuid, Uuid};

    #[test]
    fn string_delete_token() {
        let delete_token = DeleteToken::from_existing("blah");

        assert_eq!(
            delete_token,
            DeleteToken {
                id: MaybeUuid::Name(String::from("blah"))
            }
        )
    }

    #[test]
    fn uuid_string_delete_token() {
        let uuid = Uuid::new_v4();

        let delete_token = DeleteToken::from_existing(&uuid.to_string());

        assert_eq!(
            delete_token,
            DeleteToken {
                id: MaybeUuid::Uuid(uuid),
            }
        )
    }

    #[test]
    fn bytes_delete_token() {
        let delete_token = DeleteToken::from_slice(b"blah").unwrap();

        assert_eq!(
            delete_token,
            DeleteToken {
                id: MaybeUuid::Name(String::from("blah"))
            }
        )
    }

    #[test]
    fn uuid_bytes_delete_token() {
        let uuid = Uuid::new_v4();

        let delete_token = DeleteToken::from_slice(&uuid.as_bytes()[..]).unwrap();

        assert_eq!(
            delete_token,
            DeleteToken {
                id: MaybeUuid::Uuid(uuid),
            }
        )
    }

    #[test]
    fn uuid_bytes_string_delete_token() {
        let uuid = Uuid::new_v4();

        let delete_token = DeleteToken::from_slice(uuid.to_string().as_bytes()).unwrap();

        assert_eq!(
            delete_token,
            DeleteToken {
                id: MaybeUuid::Uuid(uuid),
            }
        )
    }

    #[test]
    fn string_alias() {
        let alias = Alias::from_existing("blah");

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Name(String::from("blah")),
                extension: None
            }
        );
    }

    #[test]
    fn string_alias_ext() {
        let alias = Alias::from_existing("blah.mp4");

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Name(String::from("blah")),
                extension: Some(String::from(".mp4")),
            }
        );
    }

    #[test]
    fn uuid_string_alias() {
        let uuid = Uuid::new_v4();

        let alias = Alias::from_existing(&uuid.to_string());

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Uuid(uuid),
                extension: None,
            }
        )
    }

    #[test]
    fn uuid_string_alias_ext() {
        let uuid = Uuid::new_v4();

        let alias_str = format!("{}.mp4", uuid);
        let alias = Alias::from_existing(&alias_str);

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Uuid(uuid),
                extension: Some(String::from(".mp4")),
            }
        )
    }

    #[test]
    fn bytes_alias() {
        let alias = Alias::from_slice(b"blah").unwrap();

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Name(String::from("blah")),
                extension: None
            }
        );
    }

    #[test]
    fn bytes_alias_ext() {
        let alias = Alias::from_slice(b"blah.mp4").unwrap();

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Name(String::from("blah")),
                extension: Some(String::from(".mp4")),
            }
        );
    }

    #[test]
    fn uuid_bytes_alias() {
        let uuid = Uuid::new_v4();

        let alias = Alias::from_slice(&uuid.as_bytes()[..]).unwrap();

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Uuid(uuid),
                extension: None,
            }
        )
    }

    #[test]
    fn uuid_bytes_string_alias() {
        let uuid = Uuid::new_v4();

        let alias = Alias::from_slice(uuid.to_string().as_bytes()).unwrap();

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Uuid(uuid),
                extension: None,
            }
        )
    }

    #[test]
    fn uuid_bytes_alias_ext() {
        let uuid = Uuid::new_v4();

        let mut alias_bytes = uuid.as_bytes().to_vec();
        alias_bytes.extend_from_slice(b".mp4");

        let alias = Alias::from_slice(&alias_bytes).unwrap();

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Uuid(uuid),
                extension: Some(String::from(".mp4")),
            }
        )
    }

    #[test]
    fn uuid_bytes_string_alias_ext() {
        let uuid = Uuid::new_v4();

        let alias_str = format!("{}.mp4", uuid);
        let alias = Alias::from_slice(alias_str.as_bytes()).unwrap();

        assert_eq!(
            alias,
            Alias {
                id: MaybeUuid::Uuid(uuid),
                extension: Some(String::from(".mp4")),
            }
        )
    }
}
