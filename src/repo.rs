use crate::{
    config,
    details::Details,
    error::Error,
    store::{file_store::FileId, Identifier},
};
use futures_util::Stream;
use std::{fmt::Debug, path::PathBuf};
use tracing::Instrument;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UploadId {
    id: Uuid,
}

pub(crate) enum UploadResult {
    Success { alias: Alias, token: DeleteToken },
    Failure { message: String },
}

#[async_trait::async_trait(?Send)]
pub(crate) trait FullRepo:
    CachedRepo
    + UploadRepo
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
    #[tracing::instrument(skip(self))]
    async fn identifier_from_alias<I: Identifier + 'static>(
        &self,
        alias: &Alias,
    ) -> Result<I, Error> {
        let hash = self.hash(alias).await?;
        self.identifier(hash).await
    }

    #[tracing::instrument(skip(self))]
    async fn aliases_from_alias(&self, alias: &Alias) -> Result<Vec<Alias>, Error> {
        let hash = self.hash(alias).await?;
        self.aliases(hash).await
    }

    #[tracing::instrument(skip(self))]
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

    #[tracing::instrument(skip(self))]
    async fn check_cached(&self, alias: &Alias) -> Result<(), Error> {
        let aliases = CachedRepo::update(self, alias).await?;

        for alias in aliases {
            let token = self.delete_token(&alias).await?;
            crate::queue::cleanup_alias(self, alias, token).await?;
        }

        Ok(())
    }
}

impl<T> FullRepo for actix_web::web::Data<T> where T: FullRepo {}

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
pub(crate) trait CachedRepo: BaseRepo {
    async fn mark_cached(&self, alias: &Alias) -> Result<(), Error>;

    async fn update(&self, alias: &Alias) -> Result<Vec<Alias>, Error>;
}

#[async_trait::async_trait(?Send)]
impl<T> CachedRepo for actix_web::web::Data<T>
where
    T: CachedRepo,
{
    async fn mark_cached(&self, alias: &Alias) -> Result<(), Error> {
        T::mark_cached(self, alias).await
    }

    async fn update(&self, alias: &Alias) -> Result<Vec<Alias>, Error> {
        T::update(self, alias).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait UploadRepo: BaseRepo {
    async fn create(&self, upload_id: UploadId) -> Result<(), Error>;

    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, Error>;

    async fn claim(&self, upload_id: UploadId) -> Result<(), Error>;

    async fn complete(&self, upload_id: UploadId, result: UploadResult) -> Result<(), Error>;
}

#[async_trait::async_trait(?Send)]
impl<T> UploadRepo for actix_web::web::Data<T>
where
    T: UploadRepo,
{
    async fn create(&self, upload_id: UploadId) -> Result<(), Error> {
        T::create(self, upload_id).await
    }

    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, Error> {
        T::wait(self, upload_id).await
    }

    async fn claim(&self, upload_id: UploadId) -> Result<(), Error> {
        T::claim(self, upload_id).await
    }

    async fn complete(&self, upload_id: UploadId, result: UploadResult) -> Result<(), Error> {
        T::complete(self, upload_id, result).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait QueueRepo: BaseRepo {
    async fn requeue_in_progress(&self, worker_prefix: Vec<u8>) -> Result<(), Error>;

    async fn push(&self, queue: &'static str, job: Self::Bytes) -> Result<(), Error>;

    async fn pop(&self, queue: &'static str, worker_id: Vec<u8>) -> Result<Self::Bytes, Error>;
}

#[async_trait::async_trait(?Send)]
impl<T> QueueRepo for actix_web::web::Data<T>
where
    T: QueueRepo,
{
    async fn requeue_in_progress(&self, worker_prefix: Vec<u8>) -> Result<(), Error> {
        T::requeue_in_progress(self, worker_prefix).await
    }

    async fn push(&self, queue: &'static str, job: Self::Bytes) -> Result<(), Error> {
        T::push(self, queue, job).await
    }

    async fn pop(&self, queue: &'static str, worker_id: Vec<u8>) -> Result<Self::Bytes, Error> {
        T::pop(self, queue, worker_id).await
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait SettingsRepo: BaseRepo {
    async fn set(&self, key: &'static str, value: Self::Bytes) -> Result<(), Error>;
    async fn get(&self, key: &'static str) -> Result<Option<Self::Bytes>, Error>;
    async fn remove(&self, key: &'static str) -> Result<(), Error>;
}

#[async_trait::async_trait(?Send)]
impl<T> SettingsRepo for actix_web::web::Data<T>
where
    T: SettingsRepo,
{
    async fn set(&self, key: &'static str, value: Self::Bytes) -> Result<(), Error> {
        T::set(self, key, value).await
    }

    async fn get(&self, key: &'static str) -> Result<Option<Self::Bytes>, Error> {
        T::get(self, key).await
    }

    async fn remove(&self, key: &'static str) -> Result<(), Error> {
        T::remove(self, key).await
    }
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
impl<T> IdentifierRepo for actix_web::web::Data<T>
where
    T: IdentifierRepo,
{
    async fn relate_details<I: Identifier>(
        &self,
        identifier: &I,
        details: &Details,
    ) -> Result<(), Error> {
        T::relate_details(self, identifier, details).await
    }

    async fn details<I: Identifier>(&self, identifier: &I) -> Result<Option<Details>, Error> {
        T::details(self, identifier).await
    }

    async fn cleanup<I: Identifier>(&self, identifier: &I) -> Result<(), Error> {
        T::cleanup(self, identifier).await
    }
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
    async fn remove_variant(&self, hash: Self::Bytes, variant: String) -> Result<(), Error>;

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
impl<T> HashRepo for actix_web::web::Data<T>
where
    T: HashRepo,
{
    type Stream = T::Stream;

    async fn hashes(&self) -> Self::Stream {
        T::hashes(self).await
    }

    async fn create(&self, hash: Self::Bytes) -> Result<Result<(), AlreadyExists>, Error> {
        T::create(self, hash).await
    }

    async fn relate_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Error> {
        T::relate_alias(self, hash, alias).await
    }

    async fn remove_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Error> {
        T::remove_alias(self, hash, alias).await
    }

    async fn aliases(&self, hash: Self::Bytes) -> Result<Vec<Alias>, Error> {
        T::aliases(self, hash).await
    }

    async fn relate_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Error> {
        T::relate_identifier(self, hash, identifier).await
    }

    async fn identifier<I: Identifier + 'static>(&self, hash: Self::Bytes) -> Result<I, Error> {
        T::identifier(self, hash).await
    }

    async fn relate_variant_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        variant: String,
        identifier: &I,
    ) -> Result<(), Error> {
        T::relate_variant_identifier(self, hash, variant, identifier).await
    }

    async fn variant_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
        variant: String,
    ) -> Result<Option<I>, Error> {
        T::variant_identifier(self, hash, variant).await
    }

    async fn variants<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Vec<(String, I)>, Error> {
        T::variants(self, hash).await
    }

    async fn remove_variant(&self, hash: Self::Bytes, variant: String) -> Result<(), Error> {
        T::remove_variant(self, hash, variant).await
    }

    async fn relate_motion_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Error> {
        T::relate_motion_identifier(self, hash, identifier).await
    }

    async fn motion_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, Error> {
        T::motion_identifier(self, hash).await
    }

    async fn cleanup(&self, hash: Self::Bytes) -> Result<(), Error> {
        T::cleanup(self, hash).await
    }
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

#[async_trait::async_trait(?Send)]
impl<T> AliasRepo for actix_web::web::Data<T>
where
    T: AliasRepo,
{
    async fn create(&self, alias: &Alias) -> Result<Result<(), AlreadyExists>, Error> {
        T::create(self, alias).await
    }

    async fn relate_delete_token(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
    ) -> Result<Result<(), AlreadyExists>, Error> {
        T::relate_delete_token(self, alias, delete_token).await
    }

    async fn delete_token(&self, alias: &Alias) -> Result<DeleteToken, Error> {
        T::delete_token(self, alias).await
    }

    async fn relate_hash(&self, alias: &Alias, hash: Self::Bytes) -> Result<(), Error> {
        T::relate_hash(self, alias, hash).await
    }

    async fn hash(&self, alias: &Alias) -> Result<Self::Bytes, Error> {
        T::hash(self, alias).await
    }

    async fn cleanup(&self, alias: &Alias) -> Result<(), Error> {
        T::cleanup(self, alias).await
    }
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

    pub(crate) async fn from_db(&self, path: PathBuf) -> color_eyre::Result<()> {
        if self.has_migrated().await? {
            return Ok(());
        }

        if let Some(old) = self::old::Old::open(path)? {
            let span = tracing::warn_span!("Migrating Database from 0.3 layout to 0.4 layout");

            match self {
                Self::Sled(repo) => {
                    async {
                        for hash in old.hashes() {
                            if let Err(e) = migrate_hash(repo, &old, hash).await {
                                tracing::error!("Failed to migrate hash: {}", format!("{:?}", e));
                            }
                        }

                        if let Ok(Some(value)) = old.setting(STORE_MIGRATION_PROGRESS.as_bytes()) {
                            tracing::warn!("Setting STORE_MIGRATION_PROGRESS");
                            let _ = repo.set(STORE_MIGRATION_PROGRESS, value).await;
                        }

                        if let Ok(Some(value)) = old.setting(GENERATOR_KEY.as_bytes()) {
                            tracing::warn!("Setting GENERATOR_KEY");
                            let _ = repo.set(GENERATOR_KEY, value).await;
                        }
                    }
                    .instrument(span)
                    .await;
                }
            }
        }

        self.mark_migrated().await?;

        Ok(())
    }

    pub(crate) async fn migrate_identifiers(&self) -> color_eyre::Result<()> {
        if self.has_migrated_identifiers().await? {
            return Ok(());
        }

        let span = tracing::warn_span!("Migrating File Identifiers from 0.3 format to 0.4 format");

        match self {
            Self::Sled(repo) => {
                async {
                    use futures_util::StreamExt;
                    let mut hashes = repo.hashes().await;

                    while let Some(res) = hashes.next().await {
                        let hash = res?;
                        if let Err(e) = migrate_identifiers_for_hash(repo, hash).await {
                            tracing::error!(
                                "Failed to migrate identifiers for hash: {}",
                                format!("{:?}", e)
                            );
                        }
                    }

                    Ok(()) as color_eyre::Result<()>
                }
                .instrument(span)
                .await?;
            }
        }

        self.mark_migrated_identifiers().await?;

        Ok(())
    }

    async fn has_migrated(&self) -> color_eyre::Result<bool> {
        match self {
            Self::Sled(repo) => Ok(repo.get(REPO_MIGRATION_O1).await?.is_some()),
        }
    }

    async fn has_migrated_identifiers(&self) -> color_eyre::Result<bool> {
        match self {
            Self::Sled(repo) => Ok(repo.get(REPO_MIGRATION_02).await?.is_some()),
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

    async fn mark_migrated_identifiers(&self) -> color_eyre::Result<()> {
        match self {
            Self::Sled(repo) => {
                repo.set(REPO_MIGRATION_02, b"1".to_vec().into()).await?;
            }
        }

        Ok(())
    }
}

const REPO_MIGRATION_O1: &str = "repo-migration-01";
const REPO_MIGRATION_02: &str = "repo-migration-02";
const STORE_MIGRATION_PROGRESS: &str = "store-migration-progress";
const GENERATOR_KEY: &str = "last-path";

#[tracing::instrument(skip(repo, hash), fields(hash = hex::encode(&hash)))]
async fn migrate_identifiers_for_hash<T>(repo: &T, hash: ::sled::IVec) -> color_eyre::Result<()>
where
    T: FullRepo,
{
    let hash: T::Bytes = hash.to_vec().into();

    if let Some(motion_identifier) = repo.motion_identifier::<FileId>(hash.clone()).await? {
        if let Some(new_motion_identifier) = motion_identifier.normalize_for_migration() {
            migrate_identifier_details(repo, &motion_identifier, &new_motion_identifier).await?;
            repo.relate_motion_identifier(hash.clone(), &new_motion_identifier)
                .await?;
        }
    }

    for (variant_path, variant_identifier) in repo.variants::<FileId>(hash.clone()).await? {
        if let Some(new_variant_identifier) = variant_identifier.normalize_for_migration() {
            migrate_identifier_details(repo, &variant_identifier, &new_variant_identifier).await?;
            repo.relate_variant_identifier(hash.clone(), variant_path, &new_variant_identifier)
                .await?;
        }
    }

    let main_identifier = repo.identifier::<FileId>(hash.clone()).await?;

    if let Some(new_main_identifier) = main_identifier.normalize_for_migration() {
        migrate_identifier_details(repo, &main_identifier, &new_main_identifier).await?;
        repo.relate_identifier(hash, &new_main_identifier).await?;
    }

    Ok(())
}

#[tracing::instrument(skip(repo))]
async fn migrate_identifier_details<T>(
    repo: &T,
    old: &FileId,
    new: &FileId,
) -> color_eyre::Result<()>
where
    T: FullRepo,
{
    if old == new {
        tracing::warn!("Old FileId and new FileId are identical");
        return Ok(());
    }

    if let Some(details) = repo.details(old).await? {
        repo.relate_details(new, &details).await?;
        IdentifierRepo::cleanup(repo, old).await?;
    }

    Ok(())
}

#[tracing::instrument(skip(repo, old, hash), fields(hash = hex::encode(&hash)))]
async fn migrate_hash<T>(repo: &T, old: &old::Old, hash: ::sled::IVec) -> color_eyre::Result<()>
where
    T: IdentifierRepo + HashRepo + AliasRepo + SettingsRepo + Debug,
{
    let new_hash: T::Bytes = hash.to_vec().into();
    let main_ident = old.main_identifier(&hash)?.to_vec();

    if HashRepo::create(repo, new_hash.clone()).await?.is_err() {
        tracing::warn!("Duplicate hash detected");
        return Ok(());
    }

    repo.relate_identifier(new_hash.clone(), &main_ident)
        .await?;

    for alias in old.aliases(&hash) {
        if let Ok(Ok(())) = AliasRepo::create(repo, &alias).await {
            let _ = repo.relate_alias(new_hash.clone(), &alias).await;
            let _ = repo.relate_hash(&alias, new_hash.clone()).await;

            if let Ok(Some(delete_token)) = old.delete_token(&alias) {
                let _ = repo.relate_delete_token(&alias, &delete_token).await;
            }
        }
    }

    if let Ok(Some(identifier)) = old.motion_identifier(&hash) {
        let _ = repo
            .relate_motion_identifier(new_hash.clone(), &identifier.to_vec())
            .await;
    }

    for (variant_path, identifier) in old.variants(&hash)? {
        let variant = variant_path.to_string_lossy().to_string();

        let _ = repo
            .relate_variant_identifier(new_hash.clone(), variant, &identifier.to_vec())
            .await;
    }

    for (identifier, details) in old.details(&hash)? {
        let _ = repo.relate_details(&identifier.to_vec(), &details).await;
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

    fn string_repr(&self) -> String {
        base64::encode(self.as_slice())
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
