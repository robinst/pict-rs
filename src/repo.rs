use crate::{
    config,
    details::Details,
    store::{Identifier, StoreError},
    stream::LocalBoxStream,
};
use std::{fmt::Debug, sync::Arc};
use url::Url;
use uuid::Uuid;

mod hash;
mod migrate;
pub(crate) mod sled;

pub(crate) use hash::Hash;
pub(crate) use migrate::migrate_04;

pub(crate) type ArcRepo = Arc<dyn FullRepo>;

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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DeleteToken {
    id: MaybeUuid,
}

#[derive(Debug)]
pub(crate) struct HashAlreadyExists;
#[derive(Debug)]
pub(crate) struct AliasAlreadyExists;
#[derive(Debug)]
pub(crate) struct VariantAlreadyExists;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UploadId {
    id: Uuid,
}

pub(crate) enum UploadResult {
    Success { alias: Alias, token: DeleteToken },
    Failure { message: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RepoError {
    #[error("Error in sled")]
    SledError(#[from] crate::repo::sled::SledError),

    #[error("Upload was already claimed")]
    AlreadyClaimed,

    #[error("Panic in blocking operation")]
    Canceled,
}

#[async_trait::async_trait(?Send)]
pub(crate) trait FullRepo:
    UploadRepo
    + SettingsRepo
    + DetailsRepo
    + AliasRepo
    + QueueRepo
    + HashRepo
    + StoreMigrationRepo
    + AliasAccessRepo
    + VariantAccessRepo
    + ProxyRepo
    + Send
    + Sync
    + Debug
{
    async fn health_check(&self) -> Result<(), RepoError>;

    #[tracing::instrument(skip(self))]
    async fn identifier_from_alias(&self, alias: &Alias) -> Result<Option<Arc<[u8]>>, RepoError> {
        let Some(hash) = self.hash(alias).await? else {
            return Ok(None);
        };

        self.identifier(hash).await
    }

    #[tracing::instrument(skip(self))]
    async fn aliases_from_alias(&self, alias: &Alias) -> Result<Vec<Alias>, RepoError> {
        let Some(hash) = self.hash(alias).await? else {
            return Ok(vec![]);
        };

        self.for_hash(hash).await
    }

    #[tracing::instrument(skip(self))]
    async fn still_identifier_from_alias(
        &self,
        alias: &Alias,
    ) -> Result<Option<Arc<[u8]>>, StoreError> {
        let Some(hash) = self.hash(alias).await? else {
            return Ok(None);
        };

        let Some(identifier) = self.identifier(hash.clone()).await? else {
            return Ok(None);
        };

        match self.details(&identifier).await? {
            Some(details) if details.is_video() => {
                self.motion_identifier(hash).await.map_err(From::from)
            }
            Some(_) => Ok(Some(identifier)),
            None => Ok(None),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<T> FullRepo for Arc<T>
where
    T: FullRepo,
{
    async fn health_check(&self) -> Result<(), RepoError> {
        T::health_check(self).await
    }
}

pub(crate) trait BaseRepo {}

impl<T> BaseRepo for Arc<T> where T: BaseRepo {}

#[async_trait::async_trait(?Send)]
pub(crate) trait ProxyRepo: BaseRepo {
    async fn relate_url(&self, url: Url, alias: Alias) -> Result<(), RepoError>;

    async fn related(&self, url: Url) -> Result<Option<Alias>, RepoError>;

    async fn remove_relation(&self, alias: Alias) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> ProxyRepo for Arc<T>
where
    T: ProxyRepo,
{
    async fn relate_url(&self, url: Url, alias: Alias) -> Result<(), RepoError> {
        T::relate_url(self, url, alias).await
    }

    async fn related(&self, url: Url) -> Result<Option<Alias>, RepoError> {
        T::related(self, url).await
    }

    async fn remove_relation(&self, alias: Alias) -> Result<(), RepoError> {
        T::remove_relation(self, alias).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait AliasAccessRepo: BaseRepo {
    async fn accessed_alias(&self, alias: Alias) -> Result<(), RepoError> {
        self.set_accessed_alias(alias, time::OffsetDateTime::now_utc())
            .await
    }

    async fn set_accessed_alias(
        &self,
        alias: Alias,
        accessed: time::OffsetDateTime,
    ) -> Result<(), RepoError>;

    async fn alias_accessed_at(
        &self,
        alias: Alias,
    ) -> Result<Option<time::OffsetDateTime>, RepoError>;

    async fn older_aliases(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<Alias, RepoError>>, RepoError>;

    async fn remove_alias_access(&self, alias: Alias) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> AliasAccessRepo for Arc<T>
where
    T: AliasAccessRepo,
{
    async fn set_accessed_alias(
        &self,
        alias: Alias,
        accessed: time::OffsetDateTime,
    ) -> Result<(), RepoError> {
        T::set_accessed_alias(self, alias, accessed).await
    }

    async fn alias_accessed_at(
        &self,
        alias: Alias,
    ) -> Result<Option<time::OffsetDateTime>, RepoError> {
        T::alias_accessed_at(self, alias).await
    }

    async fn older_aliases(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<Alias, RepoError>>, RepoError> {
        T::older_aliases(self, timestamp).await
    }

    async fn remove_alias_access(&self, alias: Alias) -> Result<(), RepoError> {
        T::remove_alias_access(self, alias).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait VariantAccessRepo: BaseRepo {
    async fn accessed_variant(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        self.set_accessed_variant(hash, variant, time::OffsetDateTime::now_utc())
            .await
    }

    async fn set_accessed_variant(
        &self,
        hash: Hash,
        variant: String,
        accessed: time::OffsetDateTime,
    ) -> Result<(), RepoError>;

    async fn variant_accessed_at(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<time::OffsetDateTime>, RepoError>;

    async fn older_variants(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<(Hash, String), RepoError>>, RepoError>;

    async fn remove_variant_access(&self, hash: Hash, variant: String) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> VariantAccessRepo for Arc<T>
where
    T: VariantAccessRepo,
{
    async fn set_accessed_variant(
        &self,
        hash: Hash,
        variant: String,
        accessed: time::OffsetDateTime,
    ) -> Result<(), RepoError> {
        T::set_accessed_variant(self, hash, variant, accessed).await
    }

    async fn variant_accessed_at(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<time::OffsetDateTime>, RepoError> {
        T::variant_accessed_at(self, hash, variant).await
    }

    async fn older_variants(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<(Hash, String), RepoError>>, RepoError> {
        T::older_variants(self, timestamp).await
    }

    async fn remove_variant_access(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        T::remove_variant_access(self, hash, variant).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait UploadRepo: BaseRepo {
    async fn create_upload(&self, upload_id: UploadId) -> Result<(), RepoError>;

    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, RepoError>;

    async fn claim(&self, upload_id: UploadId) -> Result<(), RepoError>;

    async fn complete(&self, upload_id: UploadId, result: UploadResult) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> UploadRepo for Arc<T>
where
    T: UploadRepo,
{
    async fn create_upload(&self, upload_id: UploadId) -> Result<(), RepoError> {
        T::create_upload(self, upload_id).await
    }

    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, RepoError> {
        T::wait(self, upload_id).await
    }

    async fn claim(&self, upload_id: UploadId) -> Result<(), RepoError> {
        T::claim(self, upload_id).await
    }

    async fn complete(&self, upload_id: UploadId, result: UploadResult) -> Result<(), RepoError> {
        T::complete(self, upload_id, result).await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct JobId(Uuid);

impl JobId {
    pub(crate) fn gen() -> Self {
        Self(Uuid::now_v7())
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    pub(crate) const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait QueueRepo: BaseRepo {
    async fn push(&self, queue: &'static str, job: Arc<[u8]>) -> Result<JobId, RepoError>;

    async fn pop(
        &self,
        queue: &'static str,
        worker_id: Uuid,
    ) -> Result<(JobId, Arc<[u8]>), RepoError>;

    async fn heartbeat(
        &self,
        queue: &'static str,
        worker_id: Uuid,
        job_id: JobId,
    ) -> Result<(), RepoError>;

    async fn complete_job(
        &self,
        queue: &'static str,
        worker_id: Uuid,
        job_id: JobId,
    ) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> QueueRepo for Arc<T>
where
    T: QueueRepo,
{
    async fn push(&self, queue: &'static str, job: Arc<[u8]>) -> Result<JobId, RepoError> {
        T::push(self, queue, job).await
    }

    async fn pop(
        &self,
        queue: &'static str,
        worker_id: Uuid,
    ) -> Result<(JobId, Arc<[u8]>), RepoError> {
        T::pop(self, queue, worker_id).await
    }

    async fn heartbeat(
        &self,
        queue: &'static str,
        worker_id: Uuid,
        job_id: JobId,
    ) -> Result<(), RepoError> {
        T::heartbeat(self, queue, worker_id, job_id).await
    }

    async fn complete_job(
        &self,
        queue: &'static str,
        worker_id: Uuid,
        job_id: JobId,
    ) -> Result<(), RepoError> {
        T::complete_job(self, queue, worker_id, job_id).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait SettingsRepo: BaseRepo {
    async fn set(&self, key: &'static str, value: Arc<[u8]>) -> Result<(), RepoError>;
    async fn get(&self, key: &'static str) -> Result<Option<Arc<[u8]>>, RepoError>;
    async fn remove(&self, key: &'static str) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> SettingsRepo for Arc<T>
where
    T: SettingsRepo,
{
    async fn set(&self, key: &'static str, value: Arc<[u8]>) -> Result<(), RepoError> {
        T::set(self, key, value).await
    }

    async fn get(&self, key: &'static str) -> Result<Option<Arc<[u8]>>, RepoError> {
        T::get(self, key).await
    }

    async fn remove(&self, key: &'static str) -> Result<(), RepoError> {
        T::remove(self, key).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait DetailsRepo: BaseRepo {
    async fn relate_details(
        &self,
        identifier: &dyn Identifier,
        details: &Details,
    ) -> Result<(), StoreError>;
    async fn details(&self, identifier: &dyn Identifier) -> Result<Option<Details>, StoreError>;

    async fn cleanup_details(&self, identifier: &dyn Identifier) -> Result<(), StoreError>;
}

#[async_trait::async_trait(?Send)]
impl<T> DetailsRepo for Arc<T>
where
    T: DetailsRepo,
{
    async fn relate_details(
        &self,
        identifier: &dyn Identifier,
        details: &Details,
    ) -> Result<(), StoreError> {
        T::relate_details(self, identifier, details).await
    }

    async fn details(&self, identifier: &dyn Identifier) -> Result<Option<Details>, StoreError> {
        T::details(self, identifier).await
    }

    async fn cleanup_details(&self, identifier: &dyn Identifier) -> Result<(), StoreError> {
        T::cleanup_details(self, identifier).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait StoreMigrationRepo: BaseRepo {
    async fn is_continuing_migration(&self) -> Result<bool, RepoError>;

    async fn mark_migrated(
        &self,
        old_identifier: &dyn Identifier,
        new_identifier: &dyn Identifier,
    ) -> Result<(), StoreError>;

    async fn is_migrated(&self, identifier: &dyn Identifier) -> Result<bool, StoreError>;

    async fn clear(&self) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> StoreMigrationRepo for Arc<T>
where
    T: StoreMigrationRepo,
{
    async fn is_continuing_migration(&self) -> Result<bool, RepoError> {
        T::is_continuing_migration(self).await
    }

    async fn mark_migrated(
        &self,
        old_identifier: &dyn Identifier,
        new_identifier: &dyn Identifier,
    ) -> Result<(), StoreError> {
        T::mark_migrated(self, old_identifier, new_identifier).await
    }

    async fn is_migrated(&self, identifier: &dyn Identifier) -> Result<bool, StoreError> {
        T::is_migrated(self, identifier).await
    }

    async fn clear(&self) -> Result<(), RepoError> {
        T::clear(self).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait HashRepo: BaseRepo {
    async fn size(&self) -> Result<u64, RepoError>;

    async fn hashes(&self) -> LocalBoxStream<'static, Result<Hash, RepoError>>;

    async fn create_hash(
        &self,
        hash: Hash,
        identifier: &dyn Identifier,
    ) -> Result<Result<(), HashAlreadyExists>, StoreError>;

    async fn update_identifier(
        &self,
        hash: Hash,
        identifier: &dyn Identifier,
    ) -> Result<(), StoreError>;

    async fn identifier(&self, hash: Hash) -> Result<Option<Arc<[u8]>>, RepoError>;

    async fn relate_variant_identifier(
        &self,
        hash: Hash,
        variant: String,
        identifier: &dyn Identifier,
    ) -> Result<Result<(), VariantAlreadyExists>, StoreError>;
    async fn variant_identifier(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<Arc<[u8]>>, RepoError>;
    async fn variants(&self, hash: Hash) -> Result<Vec<(String, Arc<[u8]>)>, RepoError>;
    async fn remove_variant(&self, hash: Hash, variant: String) -> Result<(), RepoError>;

    async fn relate_motion_identifier(
        &self,
        hash: Hash,
        identifier: &dyn Identifier,
    ) -> Result<(), StoreError>;
    async fn motion_identifier(&self, hash: Hash) -> Result<Option<Arc<[u8]>>, RepoError>;

    async fn cleanup_hash(&self, hash: Hash) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> HashRepo for Arc<T>
where
    T: HashRepo,
{
    async fn size(&self) -> Result<u64, RepoError> {
        T::size(self).await
    }

    async fn hashes(&self) -> LocalBoxStream<'static, Result<Hash, RepoError>> {
        T::hashes(self).await
    }

    async fn create_hash(
        &self,
        hash: Hash,
        identifier: &dyn Identifier,
    ) -> Result<Result<(), HashAlreadyExists>, StoreError> {
        T::create_hash(self, hash, identifier).await
    }

    async fn update_identifier(
        &self,
        hash: Hash,
        identifier: &dyn Identifier,
    ) -> Result<(), StoreError> {
        T::update_identifier(self, hash, identifier).await
    }

    async fn identifier(&self, hash: Hash) -> Result<Option<Arc<[u8]>>, RepoError> {
        T::identifier(self, hash).await
    }

    async fn relate_variant_identifier(
        &self,
        hash: Hash,
        variant: String,
        identifier: &dyn Identifier,
    ) -> Result<Result<(), VariantAlreadyExists>, StoreError> {
        T::relate_variant_identifier(self, hash, variant, identifier).await
    }

    async fn variant_identifier(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<Arc<[u8]>>, RepoError> {
        T::variant_identifier(self, hash, variant).await
    }

    async fn variants(&self, hash: Hash) -> Result<Vec<(String, Arc<[u8]>)>, RepoError> {
        T::variants(self, hash).await
    }

    async fn remove_variant(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        T::remove_variant(self, hash, variant).await
    }

    async fn relate_motion_identifier(
        &self,
        hash: Hash,
        identifier: &dyn Identifier,
    ) -> Result<(), StoreError> {
        T::relate_motion_identifier(self, hash, identifier).await
    }

    async fn motion_identifier(&self, hash: Hash) -> Result<Option<Arc<[u8]>>, RepoError> {
        T::motion_identifier(self, hash).await
    }

    async fn cleanup_hash(&self, hash: Hash) -> Result<(), RepoError> {
        T::cleanup_hash(self, hash).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait AliasRepo: BaseRepo {
    async fn create_alias(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
        hash: Hash,
    ) -> Result<Result<(), AliasAlreadyExists>, RepoError>;

    async fn delete_token(&self, alias: &Alias) -> Result<Option<DeleteToken>, RepoError>;

    async fn hash(&self, alias: &Alias) -> Result<Option<Hash>, RepoError>;

    async fn for_hash(&self, hash: Hash) -> Result<Vec<Alias>, RepoError>;

    async fn cleanup_alias(&self, alias: &Alias) -> Result<(), RepoError>;
}

#[async_trait::async_trait(?Send)]
impl<T> AliasRepo for Arc<T>
where
    T: AliasRepo,
{
    async fn create_alias(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
        hash: Hash,
    ) -> Result<Result<(), AliasAlreadyExists>, RepoError> {
        T::create_alias(self, alias, delete_token, hash).await
    }

    async fn delete_token(&self, alias: &Alias) -> Result<Option<DeleteToken>, RepoError> {
        T::delete_token(self, alias).await
    }

    async fn hash(&self, alias: &Alias) -> Result<Option<Hash>, RepoError> {
        T::hash(self, alias).await
    }

    async fn for_hash(&self, hash: Hash) -> Result<Vec<Alias>, RepoError> {
        T::for_hash(self, hash).await
    }

    async fn cleanup_alias(&self, alias: &Alias) -> Result<(), RepoError> {
        T::cleanup_alias(self, alias).await
    }
}

impl Repo {
    #[tracing::instrument]
    pub(crate) fn open(config: config::Repo) -> color_eyre::Result<Self> {
        match config {
            config::Repo::Sled(config::Sled {
                path,
                cache_capacity,
                export_path,
            }) => {
                let repo = self::sled::SledRepo::build(path, cache_capacity, export_path)?;

                Ok(Self::Sled(repo))
            }
        }
    }

    pub(crate) fn to_arc(&self) -> ArcRepo {
        match self {
            Self::Sled(sled_repo) => Arc::new(sled_repo.clone()),
        }
    }
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

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut v = self.id.as_bytes().to_vec();

        if let Some(ext) = self.extension() {
            v.extend_from_slice(ext.as_bytes());
        }

        v
    }

    pub(crate) fn from_slice(bytes: &[u8]) -> Option<Self> {
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

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        self.id.as_bytes().to_vec()
    }

    pub(crate) fn from_slice(bytes: &[u8]) -> Option<Self> {
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
}

impl std::str::FromStr for UploadId {
    type Err = <Uuid as std::str::FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(UploadId { id: s.parse()? })
    }
}

impl std::fmt::Display for UploadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.id, f)
    }
}

impl std::fmt::Display for MaybeUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uuid(id) => write!(f, "{id}"),
            Self::Name(name) => write!(f, "{name}"),
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
            write!(f, "{}{ext}", self.id)
        } else {
            write!(f, "{}", self.id)
        }
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

        let alias_str = format!("{uuid}.mp4");
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

        let alias_str = format!("{uuid}.mp4");
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
