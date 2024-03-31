use crate::{
    details::HumanDate,
    error_code::{ErrorCode, OwnedErrorCode},
    future::{WithPollTimer, WithTimeout},
    serde_str::Serde,
    stream::{from_iterator, LocalBoxStream},
};
use dashmap::DashMap;
use sled::{transaction::TransactionError, Db, IVec, Transactional, Tree};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};
use tokio::sync::Notify;
use url::Url;
use uuid::Uuid;

use super::{
    hash::Hash,
    metrics::{PopMetricsGuard, PushMetricsGuard, WaitMetricsGuard},
    notification_map::{NotificationEntry, NotificationMap},
    Alias, AliasAccessRepo, AliasAlreadyExists, AliasRepo, BaseRepo, DeleteToken, Details,
    DetailsRepo, FullRepo, HashAlreadyExists, HashPage, HashRepo, JobId, JobResult, OrderedHash,
    ProxyRepo, QueueRepo, RepoError, SettingsRepo, StoreMigrationRepo, UploadId, UploadRepo,
    UploadResult, VariantAccessRepo, VariantAlreadyExists, VariantRepo,
};

macro_rules! b {
    ($self:ident.$ident:ident, $expr:expr) => {{
        let $ident = $self.$ident.clone();

        crate::sync::spawn_blocking("sled-io", move || $expr)
            .await
            .map_err(SledError::from)
            .map_err(RepoError::from)?
            .map_err(SledError::from)
            .map_err(RepoError::from)?
    }};
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SledError {
    #[error("Error in database")]
    Sled(#[from] sled::Error),

    #[error("Invalid details json")]
    Details(#[source] serde_json::Error),

    #[error("Invalid upload result json")]
    UploadResult(#[source] serde_json::Error),

    #[error("Error parsing variant key")]
    VariantKey(#[from] VariantKeyError),

    #[error("Invalid string data in db")]
    Utf8(#[source] std::str::Utf8Error),

    #[error("Invalid job json")]
    Job(#[source] serde_json::Error),

    #[error("Operation panicked")]
    Panic,

    #[error("Another process updated this value before us")]
    Conflict,
}

impl SledError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Sled(_) | Self::VariantKey(_) | Self::Utf8(_) => ErrorCode::SLED_ERROR,
            Self::Details(_) => ErrorCode::EXTRACT_DETAILS,
            Self::UploadResult(_) => ErrorCode::EXTRACT_UPLOAD_RESULT,
            Self::Job(_) => ErrorCode::EXTRACT_JOB,
            Self::Panic => ErrorCode::PANIC,
            Self::Conflict => ErrorCode::CONFLICTED_RECORD,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SledRepo {
    healthz_count: Arc<AtomicU64>,
    healthz: Tree,
    settings: Tree,
    identifier_details: Tree,
    hashes: Tree,
    hashes_inverse: Tree,
    hash_aliases: Tree,
    hash_identifiers: Tree,
    hash_variant_identifiers: Tree,
    hash_motion_identifiers: Tree,
    hash_blurhashes: Tree,
    aliases: Tree,
    alias_hashes: Tree,
    alias_delete_tokens: Tree,
    queue: Tree,
    unique_jobs: Tree,
    unique_jobs_inverse: Tree,
    job_state: Tree,
    job_retries: Tree,
    alias_access: Tree,
    inverse_alias_access: Tree,
    variant_access: Tree,
    inverse_variant_access: Tree,
    proxy: Tree,
    inverse_proxy: Tree,
    queue_notifier: Arc<RwLock<HashMap<&'static str, Arc<Notify>>>>,
    uploads: Tree,
    migration_identifiers: Tree,
    cache_capacity: u64,
    export_path: PathBuf,
    variant_process_map: DashMap<(Hash, String), time::OffsetDateTime>,
    notifications: NotificationMap,
    db: Db,
}

impl SledRepo {
    #[tracing::instrument]
    pub(crate) fn build(
        path: PathBuf,
        cache_capacity: u64,
        export_path: PathBuf,
    ) -> color_eyre::Result<Self> {
        let db = Self::open(path, cache_capacity)?;

        Ok(SledRepo {
            healthz_count: Arc::new(AtomicU64::new(0)),
            healthz: db.open_tree("pict-rs-healthz-tree")?,
            settings: db.open_tree("pict-rs-settings-tree")?,
            identifier_details: db.open_tree("pict-rs-identifier-details-tree")?,
            hashes: db.open_tree("pict-rs-hashes-tree")?,
            hashes_inverse: db.open_tree("pict-rs-hashes-inverse-tree")?,
            hash_aliases: db.open_tree("pict-rs-hash-aliases-tree")?,
            hash_identifiers: db.open_tree("pict-rs-hash-identifiers-tree")?,
            hash_variant_identifiers: db.open_tree("pict-rs-hash-variant-identifiers-tree")?,
            hash_motion_identifiers: db.open_tree("pict-rs-hash-motion-identifiers-tree")?,
            hash_blurhashes: db.open_tree("pict-rs-hash-blurhashes-tree")?,
            aliases: db.open_tree("pict-rs-aliases-tree")?,
            alias_hashes: db.open_tree("pict-rs-alias-hashes-tree")?,
            alias_delete_tokens: db.open_tree("pict-rs-alias-delete-tokens-tree")?,
            queue: db.open_tree("pict-rs-queue-tree")?,
            unique_jobs: db.open_tree("pict-rs-unique-jobs-tree")?,
            unique_jobs_inverse: db.open_tree("pict-rs-unique-jobs-inverse-tree")?,
            job_state: db.open_tree("pict-rs-job-state-tree")?,
            job_retries: db.open_tree("pict-rs-job-retries-tree")?,
            alias_access: db.open_tree("pict-rs-alias-access-tree")?,
            inverse_alias_access: db.open_tree("pict-rs-inverse-alias-access-tree")?,
            variant_access: db.open_tree("pict-rs-variant-access-tree")?,
            inverse_variant_access: db.open_tree("pict-rs-inverse-variant-access-tree")?,
            proxy: db.open_tree("pict-rs-proxy-tree")?,
            inverse_proxy: db.open_tree("pict-rs-inverse-proxy-tree")?,
            queue_notifier: Arc::new(RwLock::new(HashMap::new())),
            uploads: db.open_tree("pict-rs-uploads-tree")?,
            migration_identifiers: db.open_tree("pict-rs-migration-identifiers-tree")?,
            cache_capacity,
            export_path,
            variant_process_map: DashMap::new(),
            notifications: NotificationMap::new(),
            db,
        })
    }

    fn open(mut path: PathBuf, cache_capacity: u64) -> Result<Db, SledError> {
        path.push("v0.5.0");

        let db = ::sled::Config::new()
            .cache_capacity(cache_capacity)
            .path(path)
            .open()?;

        Ok(db)
    }

    #[tracing::instrument(level = "warn", skip_all)]
    pub(crate) async fn export(&self) -> Result<(), RepoError> {
        let path = self.export_path.join(
            HumanDate {
                timestamp: time::OffsetDateTime::now_utc(),
            }
            .to_string(),
        );

        let export_db = Self::open(path, self.cache_capacity)?;

        let this = self.db.clone();

        crate::sync::spawn_blocking("sled-io", move || {
            let export = this.export();
            export_db.import(export);
        })
        .await
        .map_err(SledError::from)?;

        Ok(())
    }
}

impl BaseRepo for SledRepo {}

#[async_trait::async_trait(?Send)]
impl FullRepo for SledRepo {
    async fn health_check(&self) -> Result<(), RepoError> {
        let next = self.healthz_count.fetch_add(1, Ordering::Relaxed);
        b!(self.healthz, {
            healthz.insert("healthz", &next.to_be_bytes()[..])
        });
        self.healthz.flush_async().await.map_err(SledError::from)?;
        b!(self.healthz, healthz.get("healthz"));
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ProxyRepo for SledRepo {
    async fn relate_url(&self, url: Url, alias: Alias) -> Result<(), RepoError> {
        let proxy = self.proxy.clone();
        let inverse_proxy = self.inverse_proxy.clone();

        crate::sync::spawn_blocking("sled-io", move || {
            proxy.insert(url.as_str().as_bytes(), alias.to_bytes())?;
            inverse_proxy.insert(alias.to_bytes(), url.as_str().as_bytes())?;

            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)??;

        Ok(())
    }

    async fn related(&self, url: Url) -> Result<Option<Alias>, RepoError> {
        let opt = b!(self.proxy, proxy.get(url.as_str().as_bytes()));

        Ok(opt.and_then(|ivec| Alias::from_slice(&ivec)))
    }

    async fn remove_relation(&self, alias: Alias) -> Result<(), RepoError> {
        let proxy = self.proxy.clone();
        let inverse_proxy = self.inverse_proxy.clone();

        crate::sync::spawn_blocking("sled-io", move || {
            if let Some(url) = inverse_proxy.remove(alias.to_bytes())? {
                proxy.remove(url)?;
            }

            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)??;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl AliasAccessRepo for SledRepo {
    #[tracing::instrument(level = "debug", skip(self))]
    async fn set_accessed_alias(
        &self,
        alias: Alias,
        accessed: time::OffsetDateTime,
    ) -> Result<(), RepoError> {
        let mut value_bytes = accessed.unix_timestamp_nanos().to_be_bytes().to_vec();
        value_bytes.extend_from_slice(&alias.to_bytes());
        let value_bytes = IVec::from(value_bytes);

        let alias_access = self.alias_access.clone();
        let inverse_alias_access = self.inverse_alias_access.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (&alias_access, &inverse_alias_access).transaction(
                |(alias_access, inverse_alias_access)| {
                    if let Some(old) = alias_access.insert(alias.to_bytes(), &value_bytes)? {
                        inverse_alias_access.remove(old)?;
                    }
                    inverse_alias_access.insert(&value_bytes, alias.to_bytes())?;

                    Ok(())
                },
            )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        if let Err(TransactionError::Abort(e) | TransactionError::Storage(e)) = res {
            return Err(RepoError::from(SledError::from(e)));
        }

        Ok(())
    }

    async fn alias_accessed_at(
        &self,
        alias: Alias,
    ) -> Result<Option<time::OffsetDateTime>, RepoError> {
        let alias = alias.to_bytes();

        let Some(timestamp) = b!(self.variant_access, variant_access.get(alias)) else {
            return Ok(None);
        };

        let timestamp = timestamp[0..16].try_into().expect("valid timestamp bytes");

        let timestamp =
            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from_be_bytes(timestamp))
                .expect("valid timestamp");

        Ok(Some(timestamp))
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn older_aliases(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<Alias, RepoError>>, RepoError> {
        let time_bytes = timestamp.unix_timestamp_nanos().to_be_bytes().to_vec();

        let iterator = self
            .inverse_alias_access
            .range(..=time_bytes)
            .filter_map(|res| {
                res.map_err(SledError::from)
                    .map_err(RepoError::from)
                    .map(|(_, value)| Alias::from_slice(&value))
                    .transpose()
            });

        Ok(Box::pin(from_iterator(iterator, 8)))
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn remove_alias_access(&self, alias: Alias) -> Result<(), RepoError> {
        let alias_access = self.alias_access.clone();
        let inverse_alias_access = self.inverse_alias_access.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (&alias_access, &inverse_alias_access).transaction(
                |(alias_access, inverse_alias_access)| {
                    if let Some(old) = alias_access.remove(alias.to_bytes())? {
                        inverse_alias_access.remove(old)?;
                    }
                    Ok(())
                },
            )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        if let Err(TransactionError::Abort(e) | TransactionError::Storage(e)) = res {
            return Err(RepoError::from(SledError::from(e)));
        }

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl VariantAccessRepo for SledRepo {
    #[tracing::instrument(level = "debug", skip(self))]
    async fn set_accessed_variant(
        &self,
        hash: Hash,
        variant: String,
        accessed: time::OffsetDateTime,
    ) -> Result<(), RepoError> {
        let hash = hash.to_bytes();
        let key = IVec::from(variant_access_key(&hash, &variant));

        let mut value_bytes = accessed.unix_timestamp_nanos().to_be_bytes().to_vec();
        value_bytes.extend_from_slice(&key);
        let value_bytes = IVec::from(value_bytes);

        let variant_access = self.variant_access.clone();
        let inverse_variant_access = self.inverse_variant_access.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (&variant_access, &inverse_variant_access).transaction(
                |(variant_access, inverse_variant_access)| {
                    if let Some(old) = variant_access.insert(&key, &value_bytes)? {
                        inverse_variant_access.remove(old)?;
                    }
                    inverse_variant_access.insert(&value_bytes, &key)?;

                    Ok(())
                },
            )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        if let Err(TransactionError::Abort(e) | TransactionError::Storage(e)) = res {
            return Err(RepoError::from(SledError::from(e)));
        }

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn variant_accessed_at(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<time::OffsetDateTime>, RepoError> {
        let hash = hash.to_bytes();
        let key = variant_access_key(&hash, &variant);

        let Some(timestamp) = b!(self.variant_access, variant_access.get(key)) else {
            return Ok(None);
        };

        let timestamp = timestamp[0..16].try_into().expect("valid timestamp bytes");

        let timestamp =
            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from_be_bytes(timestamp))
                .expect("valid timestamp");

        Ok(Some(timestamp))
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn older_variants(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<(Hash, String), RepoError>>, RepoError> {
        let time_bytes = timestamp.unix_timestamp_nanos().to_be_bytes().to_vec();

        let iterator = self.inverse_variant_access.range(..=time_bytes).map(|res| {
            let (_, bytes) = res.map_err(SledError::from)?;

            parse_variant_access_key(bytes)
                .map_err(SledError::from)
                .map_err(RepoError::from)
        });

        Ok(Box::pin(from_iterator(iterator, 8)))
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn remove_variant_access(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        let hash = hash.to_bytes();
        let key = IVec::from(variant_access_key(&hash, &variant));

        let variant_access = self.variant_access.clone();
        let inverse_variant_access = self.inverse_variant_access.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (&variant_access, &inverse_variant_access).transaction(
                |(variant_access, inverse_variant_access)| {
                    if let Some(old) = variant_access.remove(&key)? {
                        inverse_variant_access.remove(old)?;
                    }

                    Ok(())
                },
            )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        if let Err(TransactionError::Abort(e) | TransactionError::Storage(e)) = res {
            return Err(RepoError::from(SledError::from(e)));
        }

        Ok(())
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
enum InnerUploadResult {
    Success {
        alias: Serde<Alias>,
        token: Serde<DeleteToken>,
    },
    Failure {
        message: String,
        code: OwnedErrorCode,
    },
}

impl From<UploadResult> for InnerUploadResult {
    fn from(u: UploadResult) -> Self {
        match u {
            UploadResult::Success { alias, token } => InnerUploadResult::Success {
                alias: Serde::new(alias),
                token: Serde::new(token),
            },
            UploadResult::Failure { message, code } => InnerUploadResult::Failure { message, code },
        }
    }
}

impl From<InnerUploadResult> for UploadResult {
    fn from(i: InnerUploadResult) -> Self {
        match i {
            InnerUploadResult::Success { alias, token } => UploadResult::Success {
                alias: Serde::into_inner(alias),
                token: Serde::into_inner(token),
            },
            InnerUploadResult::Failure { message, code } => UploadResult::Failure { message, code },
        }
    }
}

#[async_trait::async_trait(?Send)]
impl UploadRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn create_upload(&self) -> Result<UploadId, RepoError> {
        let upload_id = UploadId::generate();

        b!(self.uploads, uploads.insert(upload_id.as_bytes(), b"1"));

        Ok(upload_id)
    }

    #[tracing::instrument(skip(self))]
    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, RepoError> {
        let guard = WaitMetricsGuard::guard();
        let mut subscriber = self.uploads.watch_prefix(upload_id.as_bytes());

        let bytes = upload_id.as_bytes().to_vec();
        let opt = b!(self.uploads, uploads.get(bytes));

        if let Some(bytes) = opt {
            if bytes != b"1" {
                let result: InnerUploadResult =
                    serde_json::from_slice(&bytes).map_err(SledError::UploadResult)?;
                guard.disarm();
                return Ok(result.into());
            }
        } else {
            return Err(RepoError::AlreadyClaimed);
        }

        while let Some(event) = (&mut subscriber).await {
            tracing::trace!("wait: looping");

            match event {
                sled::Event::Remove { .. } => {
                    return Err(RepoError::AlreadyClaimed);
                }
                sled::Event::Insert { value, .. } => {
                    if value != b"1" {
                        let result: InnerUploadResult =
                            serde_json::from_slice(&value).map_err(SledError::UploadResult)?;

                        guard.disarm();
                        return Ok(result.into());
                    }
                }
            }
        }

        Err(RepoError::Canceled)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn claim(&self, upload_id: UploadId) -> Result<(), RepoError> {
        b!(self.uploads, uploads.remove(upload_id.as_bytes()));
        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self, result))]
    async fn complete_upload(
        &self,
        upload_id: UploadId,
        result: UploadResult,
    ) -> Result<(), RepoError> {
        let result: InnerUploadResult = result.into();
        let result = serde_json::to_vec(&result).map_err(SledError::UploadResult)?;

        b!(self.uploads, uploads.insert(upload_id.as_bytes(), result));

        Ok(())
    }
}

enum JobState {
    Pending,
    Running([u8; 24]),
}

impl JobState {
    const fn pending() -> Self {
        Self::Pending
    }

    fn running(worker_id: Uuid) -> Self {
        let first_eight = time::OffsetDateTime::now_utc()
            .unix_timestamp()
            .to_be_bytes();

        let next_sixteen = worker_id.into_bytes();

        let mut bytes = [0u8; 24];

        bytes[0..8]
            .iter_mut()
            .zip(&first_eight)
            .for_each(|(dest, src)| *dest = *src);

        bytes[8..24]
            .iter_mut()
            .zip(&next_sixteen)
            .for_each(|(dest, src)| *dest = *src);

        Self::Running(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Pending => b"pend",
            Self::Running(ref bytes) => bytes,
        }
    }
}

fn job_key(queue: &'static str, job_id: JobId) -> Arc<[u8]> {
    let mut key = queue.as_bytes().to_vec();
    key.extend(job_id.as_bytes());

    Arc::from(key)
}

fn try_into_arc_str(ivec: IVec) -> Result<Arc<str>, SledError> {
    std::str::from_utf8(&ivec[..])
        .map_err(SledError::Utf8)
        .map(String::from)
        .map(Arc::from)
}

#[async_trait::async_trait(?Send)]
impl QueueRepo for SledRepo {
    async fn queue_length(&self) -> Result<u64, RepoError> {
        let queue = self.queue.clone();

        let size = crate::sync::spawn_blocking("sled-io", move || queue.len())
            .await
            .map_err(|_| RepoError::Canceled)?;

        Ok(size as u64)
    }

    #[tracing::instrument(skip(self))]
    async fn push(
        &self,
        queue_name: &'static str,
        job: serde_json::Value,
        unique_key: Option<&'static str>,
    ) -> Result<Option<JobId>, RepoError> {
        let metrics_guard = PushMetricsGuard::guard(queue_name);

        let id = JobId::gen();
        let key = job_key(queue_name, id);
        let job = serde_json::to_vec(&job).map_err(SledError::Job)?;

        let queue = self.queue.clone();
        let unique_jobs = self.unique_jobs.clone();
        let unique_jobs_inverse = self.unique_jobs_inverse.clone();
        let job_state = self.job_state.clone();
        let job_retries = self.job_retries.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (
                &queue,
                &unique_jobs,
                &unique_jobs_inverse,
                &job_state,
                &job_retries,
            )
                .transaction(
                    |(queue, unique_jobs, unique_jobs_inverse, job_state, job_retries)| {
                        let state = JobState::pending();

                        queue.insert(&key[..], &job[..])?;
                        if let Some(unique_key) = unique_key {
                            if unique_jobs
                                .insert(unique_key.as_bytes(), &key[..])?
                                .is_some()
                            {
                                return sled::transaction::abort(());
                            }

                            unique_jobs_inverse.insert(&key[..], unique_key.as_bytes())?;
                        }
                        job_state.insert(&key[..], state.as_bytes())?;
                        job_retries.insert(&key[..], &(5_u64.to_be_bytes())[..])?;

                        Ok(())
                    },
                )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        match res {
            Err(TransactionError::Abort(())) => return Ok(None),
            Err(TransactionError::Storage(e)) => return Err(RepoError::from(SledError::from(e))),
            Ok(()) => (),
        }

        if let Some(notifier) = self.queue_notifier.read().unwrap().get(&queue_name) {
            notifier.notify_one();
            metrics_guard.disarm();
            return Ok(Some(id));
        }

        self.queue_notifier
            .write()
            .unwrap()
            .entry(queue_name)
            .or_insert_with(crate::sync::notify)
            .notify_one();

        metrics_guard.disarm();

        Ok(Some(id))
    }

    #[tracing::instrument(skip_all, fields(job_id))]
    async fn pop(
        &self,
        queue_name: &'static str,
        worker_id: Uuid,
    ) -> Result<(JobId, serde_json::Value), RepoError> {
        let metrics_guard = PopMetricsGuard::guard(queue_name);

        let now = time::OffsetDateTime::now_utc();

        loop {
            tracing::trace!("pop: looping");

            let queue = self.queue.clone();
            let job_state = self.job_state.clone();

            let opt = crate::sync::spawn_blocking("sled-io", move || {
                // Job IDs are generated with Uuid version 7 - defining their first bits as a
                // timestamp. Scanning a prefix should give us jobs in the order they were queued.
                for res in job_state.scan_prefix(queue_name) {
                    let (key, value) = res?;

                    if value.len() > 8 {
                        let unix_timestamp =
                            i64::from_be_bytes(value[0..8].try_into().expect("Verified length"));

                        let timestamp = time::OffsetDateTime::from_unix_timestamp(unix_timestamp)
                            .expect("Valid timestamp");

                        // heartbeats should update every 5 seconds, so 30 seconds without an
                        // update is 6 missed beats
                        if timestamp.saturating_add(time::Duration::seconds(30)) > now {
                            // job hasn't expired
                            continue;
                        }
                    }

                    let state = JobState::running(worker_id);

                    match job_state.compare_and_swap(&key, Some(value), Some(state.as_bytes()))? {
                        Ok(()) => {
                            // acquired job
                        }
                        Err(_) => {
                            tracing::debug!("Contested");
                            // someone else acquired job
                            continue;
                        }
                    }

                    let id_bytes = &key[queue_name.len()..];

                    let id_bytes: [u8; 16] = id_bytes.try_into().expect("Key length");

                    let job_id = JobId::from_bytes(id_bytes);

                    let opt = queue
                        .get(&key)?
                        .map(|ivec| serde_json::from_slice(&ivec[..]))
                        .transpose()
                        .map_err(SledError::Job)?;

                    return Ok(opt.map(|job| (job_id, job)))
                        as Result<Option<(JobId, serde_json::Value)>, SledError>;
                }

                Ok(None)
            })
            .with_poll_timer("sled-pop-spawn-blocking")
            .await
            .map_err(|_| RepoError::Canceled)??;

            if let Some((job_id, job_json)) = opt {
                tracing::Span::current().record("job_id", &format!("{}", job_id.0));

                metrics_guard.disarm();
                tracing::debug!("{job_json}");
                return Ok((job_id, job_json));
            }

            let opt = self
                .queue_notifier
                .read()
                .unwrap()
                .get(&queue_name)
                .map(Arc::clone);

            let notify = if let Some(notify) = opt {
                notify
            } else {
                let mut guard = self.queue_notifier.write().unwrap();
                let entry = guard.entry(queue_name).or_insert_with(crate::sync::notify);
                Arc::clone(entry)
            };

            match notify
                .notified()
                .with_timeout(Duration::from_secs(30))
                .with_poll_timer("sled-pop-notify")
                .await
            {
                Ok(()) => tracing::debug!("Notified"),
                Err(_) => tracing::trace!("Timed out"),
            }
        }
    }

    #[tracing::instrument(skip(self, worker_id))]
    async fn heartbeat(
        &self,
        queue_name: &'static str,
        worker_id: Uuid,
        job_id: JobId,
    ) -> Result<(), RepoError> {
        let key = job_key(queue_name, job_id);

        let job_state = self.job_state.clone();

        crate::sync::spawn_blocking("sled-io", move || {
            if let Some(state) = job_state.get(&key)? {
                let new_state = JobState::running(worker_id);

                match job_state.compare_and_swap(&key, Some(state), Some(new_state.as_bytes()))? {
                    Ok(()) => Ok(()),
                    Err(_) => Err(SledError::Conflict),
                }
            } else {
                Ok(())
            }
        })
        .await
        .map_err(|_| RepoError::Canceled)??;

        Ok(())
    }

    #[tracing::instrument(skip(self, _worker_id))]
    async fn complete_job(
        &self,
        queue_name: &'static str,
        _worker_id: Uuid,
        job_id: JobId,
        job_status: JobResult,
    ) -> Result<(), RepoError> {
        let retry = matches!(job_status, JobResult::Failure);

        let key = job_key(queue_name, job_id);

        let queue = self.queue.clone();
        let unique_jobs = self.unique_jobs.clone();
        let unique_jobs_inverse = self.unique_jobs_inverse.clone();
        let job_state = self.job_state.clone();
        let job_retries = self.job_retries.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (
                &queue,
                &unique_jobs,
                &unique_jobs_inverse,
                &job_state,
                &job_retries,
            )
                .transaction(
                    |(queue, unique_jobs, unique_jobs_inverse, job_state, job_retries)| {
                        let retries = job_retries.get(&key[..])?;

                        let retry_count = retries
                            .and_then(|ivec| ivec[0..8].try_into().ok())
                            .map(u64::from_be_bytes)
                            .unwrap_or(5_u64)
                            .saturating_sub(1);

                        if retry_count > 0 && retry {
                            job_retries.insert(&key[..], &(retry_count.to_be_bytes())[..])?;
                        } else {
                            queue.remove(&key[..])?;
                            if let Some(unique_key) = unique_jobs_inverse.remove(&key[..])? {
                                unique_jobs.remove(unique_key)?;
                            }
                            job_state.remove(&key[..])?;
                            job_retries.remove(&key[..])?;
                        }

                        Ok(retry_count > 0 && retry)
                    },
                )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        match res {
            Err(TransactionError::Abort(e) | TransactionError::Storage(e)) => {
                return Err(RepoError::from(SledError::from(e)));
            }
            Ok(retried) => match job_status {
                JobResult::Success => tracing::debug!("completed {job_id:?}"),
                JobResult::Failure if retried => {
                    tracing::info!("{job_id:?} failed, marked for retry")
                }
                JobResult::Failure => tracing::warn!("{job_id:?} failed permantently"),
                JobResult::Aborted => tracing::warn!("{job_id:?} dead"),
            },
        }

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl SettingsRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(value))]
    async fn set(&self, key: &'static str, value: Arc<[u8]>) -> Result<(), RepoError> {
        b!(self.settings, settings.insert(key, &value[..]));

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn get(&self, key: &'static str) -> Result<Option<Arc<[u8]>>, RepoError> {
        let opt = b!(self.settings, settings.get(key));

        Ok(opt.map(|ivec| Arc::from(ivec.to_vec())))
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn remove(&self, key: &'static str) -> Result<(), RepoError> {
        b!(self.settings, settings.remove(key));

        Ok(())
    }
}

fn variant_access_key(hash: &[u8], variant: &str) -> Vec<u8> {
    let variant = variant.as_bytes();

    let hash_len: u64 = u64::try_from(hash.len()).expect("Length is reasonable");

    let mut out = Vec::with_capacity(8 + hash.len() + variant.len());

    let hash_length_bytes: [u8; 8] = hash_len.to_be_bytes();
    out.extend(hash_length_bytes);
    out.extend(hash);
    out.extend(variant);
    out
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum VariantKeyError {
    #[error("Bytes too short to be VariantAccessKey")]
    TooShort,

    #[error("Prefix Length is longer than backing bytes")]
    InvalidLength,

    #[error("Invalid utf8 in Variant")]
    Utf8,

    #[error("Hash format is invalid")]
    InvalidHash,
}

fn parse_variant_access_key(bytes: IVec) -> Result<(Hash, String), VariantKeyError> {
    if bytes.len() < 8 {
        return Err(VariantKeyError::TooShort);
    }

    let hash_len = u64::from_be_bytes(bytes[..8].try_into().expect("Verified length"));
    let hash_len: usize = usize::try_from(hash_len).expect("Length is reasonable");

    if (hash_len + 8) > bytes.len() {
        return Err(VariantKeyError::InvalidLength);
    }

    let hash = bytes.subslice(8, hash_len);

    let hash = Hash::from_ivec(hash).ok_or(VariantKeyError::InvalidHash)?;

    let variant_len = bytes.len().saturating_sub(8).saturating_sub(hash_len);

    if variant_len == 0 {
        return Ok((hash, String::new()));
    }

    let variant_start = 8 + hash_len;

    let variant = std::str::from_utf8(&bytes[variant_start..])
        .map_err(|_| VariantKeyError::Utf8)?
        .to_string();

    Ok((hash, variant))
}

fn variant_key(hash: &[u8], variant: &str) -> Vec<u8> {
    let mut bytes = hash.to_vec();
    bytes.push(b'/');
    bytes.extend_from_slice(variant.as_bytes());
    bytes
}

fn variant_from_key(hash: &[u8], key: &[u8]) -> Option<String> {
    let prefix_len = hash.len() + 1;
    let variant_bytes = key.get(prefix_len..)?.to_vec();
    String::from_utf8(variant_bytes).ok()
}

#[async_trait::async_trait(?Send)]
impl DetailsRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn relate_details(
        &self,
        identifier: &Arc<str>,
        details: &Details,
    ) -> Result<(), RepoError> {
        let key = identifier.clone();
        let details = serde_json::to_vec(&details.inner).map_err(SledError::Details)?;

        b!(
            self.identifier_details,
            identifier_details.insert(key.as_bytes(), details)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn details(&self, identifier: &Arc<str>) -> Result<Option<Details>, RepoError> {
        let key = identifier.clone();

        let opt = b!(
            self.identifier_details,
            identifier_details.get(key.as_bytes())
        );

        opt.map(|ivec| serde_json::from_slice(&ivec).map(|inner| Details { inner }))
            .transpose()
            .map_err(SledError::Details)
            .map_err(RepoError::from)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn cleanup_details(&self, identifier: &Arc<str>) -> Result<(), RepoError> {
        let key = identifier.clone();

        b!(
            self.identifier_details,
            identifier_details.remove(key.as_bytes())
        );

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl StoreMigrationRepo for SledRepo {
    async fn is_continuing_migration(&self) -> Result<bool, RepoError> {
        Ok(!self.migration_identifiers.is_empty())
    }

    async fn mark_migrated(
        &self,
        old_identifier: &Arc<str>,
        new_identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        let key = new_identifier.clone();
        let value = old_identifier.clone();

        b!(
            self.migration_identifiers,
            migration_identifiers.insert(key.as_bytes(), value.as_bytes())
        );

        Ok(())
    }

    async fn is_migrated(&self, identifier: &Arc<str>) -> Result<bool, RepoError> {
        let key = identifier.clone();

        Ok(b!(
            self.migration_identifiers,
            migration_identifiers.get(key.as_bytes())
        )
        .is_some())
    }

    async fn clear(&self) -> Result<(), RepoError> {
        b!(self.migration_identifiers, migration_identifiers.clear());

        Ok(())
    }
}

fn parse_ordered_hash(key: IVec) -> Option<OrderedHash> {
    if key.len() < 16 {
        return None;
    }

    let timestamp_bytes: [u8; 16] = key[0..16].try_into().ok()?;
    let timestamp_i128 = i128::from_be_bytes(timestamp_bytes);
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(timestamp_i128).ok()?;

    let hash = Hash::from_bytes(&key[16..])?;

    Some(OrderedHash { timestamp, hash })
}

fn serialize_ordered_hash(ordered_hash: &OrderedHash) -> IVec {
    let mut bytes: Vec<u8> = ordered_hash
        .timestamp
        .unix_timestamp_nanos()
        .to_be_bytes()
        .into();

    bytes.extend(ordered_hash.hash.to_bytes());

    IVec::from(bytes)
}

#[async_trait::async_trait(?Send)]
impl HashRepo for SledRepo {
    async fn size(&self) -> Result<u64, RepoError> {
        Ok(b!(
            self.hashes,
            Ok(u64::try_from(hashes.len()).expect("Length is reasonable"))
                as Result<u64, SledError>
        ))
    }

    async fn bound(&self, hash: Hash) -> Result<Option<OrderedHash>, RepoError> {
        let opt = b!(self.hashes, hashes.get(hash.to_ivec()));

        Ok(opt.and_then(parse_ordered_hash))
    }

    async fn hashes_ordered(
        &self,
        bound: Option<OrderedHash>,
        limit: usize,
    ) -> Result<HashPage, RepoError> {
        let (page_iter, prev_iter) = match &bound {
            Some(ordered_hash) => {
                let hash_bytes = serialize_ordered_hash(ordered_hash);
                (
                    self.hashes_inverse.range(..=hash_bytes.clone()),
                    Some(self.hashes_inverse.range(hash_bytes..)),
                )
            }
            None => (self.hashes_inverse.iter(), None),
        };

        crate::sync::spawn_blocking("sled-io", move || {
            let page_iter = page_iter
                .keys()
                .rev()
                .filter_map(|res| res.map(parse_ordered_hash).transpose())
                .take(limit + 1);

            let prev = prev_iter
                .and_then(|prev_iter| {
                    prev_iter
                        .keys()
                        .filter_map(|res| res.map(parse_ordered_hash).transpose())
                        .take(limit + 1)
                        .last()
                })
                .transpose()?;

            let mut hashes = page_iter.collect::<Result<Vec<_>, _>>()?;

            let next = if hashes.len() > limit {
                hashes.pop()
            } else {
                None
            };

            let prev = if prev == bound { None } else { prev };

            Ok(HashPage {
                limit,
                prev: prev.map(|OrderedHash { hash, .. }| hash),
                next: next.map(|OrderedHash { hash, .. }| hash),
                hashes: hashes
                    .into_iter()
                    .map(|OrderedHash { hash, .. }| hash)
                    .collect(),
            }) as Result<HashPage, SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
    }

    async fn hash_page_by_date(
        &self,
        date: time::OffsetDateTime,
        limit: usize,
    ) -> Result<HashPage, RepoError> {
        let date_nanos = date.unix_timestamp_nanos().to_be_bytes();

        let page_iter = self.hashes_inverse.range(..=date_nanos);
        let prev_iter = Some(self.hashes_inverse.range(date_nanos..));

        crate::sync::spawn_blocking("sled-io", move || {
            let page_iter = page_iter
                .keys()
                .rev()
                .filter_map(|res| res.map(parse_ordered_hash).transpose())
                .take(limit + 1);

            let prev = prev_iter
                .and_then(|prev_iter| {
                    prev_iter
                        .keys()
                        .filter_map(|res| res.map(parse_ordered_hash).transpose())
                        .take(limit + 1)
                        .last()
                })
                .transpose()?;

            let mut hashes = page_iter.collect::<Result<Vec<_>, _>>()?;

            let next = if hashes.len() > limit {
                hashes.pop()
            } else {
                None
            };

            let prev = if prev.as_ref() == hashes.first() {
                None
            } else {
                prev
            };

            Ok(HashPage {
                limit,
                prev: prev.map(|OrderedHash { hash, .. }| hash),
                next: next.map(|OrderedHash { hash, .. }| hash),
                hashes: hashes
                    .into_iter()
                    .map(|OrderedHash { hash, .. }| hash)
                    .collect(),
            }) as Result<HashPage, SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn create_hash_with_timestamp(
        &self,
        hash: Hash,
        identifier: &Arc<str>,
        timestamp: time::OffsetDateTime,
    ) -> Result<Result<(), HashAlreadyExists>, RepoError> {
        let identifier: sled::IVec = identifier.as_bytes().to_vec().into();

        let hashes = self.hashes.clone();
        let hashes_inverse = self.hashes_inverse.clone();
        let hash_identifiers = self.hash_identifiers.clone();

        let created_key = serialize_ordered_hash(&OrderedHash {
            timestamp,
            hash: hash.clone(),
        });

        let hash = hash.to_ivec();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (&hashes, &hashes_inverse, &hash_identifiers).transaction(
                |(hashes, hashes_inverse, hash_identifiers)| {
                    if hashes.get(hash.clone())?.is_some() {
                        return Ok(Err(HashAlreadyExists));
                    }

                    hashes.insert(hash.clone(), created_key.clone())?;
                    hashes_inverse.insert(created_key.clone(), hash.clone())?;
                    hash_identifiers.insert(hash.clone(), &identifier)?;

                    Ok(Ok(()))
                },
            )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        match res {
            Ok(res) => Ok(res),
            Err(TransactionError::Abort(e) | TransactionError::Storage(e)) => {
                Err(RepoError::from(SledError::from(e)))
            }
        }
    }

    async fn update_identifier(&self, hash: Hash, identifier: &Arc<str>) -> Result<(), RepoError> {
        let identifier = identifier.clone();

        let hash = hash.to_ivec();

        b!(
            self.hash_identifiers,
            hash_identifiers.insert(hash, identifier.as_bytes())
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn identifier(&self, hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        let hash = hash.to_ivec();

        let opt = b!(self.hash_identifiers, hash_identifiers.get(hash));

        Ok(opt.map(try_into_arc_str).transpose()?)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn relate_blurhash(&self, hash: Hash, blurhash: Arc<str>) -> Result<(), RepoError> {
        b!(
            self.hash_blurhashes,
            hash_blurhashes.insert(hash.to_bytes(), blurhash.as_bytes())
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn blurhash(&self, hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        let opt = b!(self.hash_blurhashes, hash_blurhashes.get(hash.to_ivec()));

        Ok(opt.map(try_into_arc_str).transpose()?)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn relate_motion_identifier(
        &self,
        hash: Hash,
        identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        let hash = hash.to_ivec();
        let bytes = identifier.clone();

        b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.insert(hash, bytes.as_bytes())
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn motion_identifier(&self, hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        let hash = hash.to_ivec();

        let opt = b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.get(hash)
        );

        Ok(opt.map(try_into_arc_str).transpose()?)
    }

    #[tracing::instrument(skip(self))]
    async fn cleanup_hash(&self, hash: Hash) -> Result<(), RepoError> {
        let hash = hash.to_ivec();

        let hashes = self.hashes.clone();
        let hashes_inverse = self.hashes_inverse.clone();
        let hash_identifiers = self.hash_identifiers.clone();
        let hash_motion_identifiers = self.hash_motion_identifiers.clone();
        let hash_variant_identifiers = self.hash_variant_identifiers.clone();
        let hash_blurhashes = self.hash_blurhashes.clone();

        let hash2 = hash.clone();
        let variant_keys = b!(self.hash_variant_identifiers, {
            let v = hash_variant_identifiers
                .scan_prefix(hash2)
                .keys()
                .filter_map(Result::ok)
                .collect::<Vec<_>>();

            Ok(v) as Result<Vec<_>, SledError>
        });

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (
                &hashes,
                &hashes_inverse,
                &hash_identifiers,
                &hash_motion_identifiers,
                &hash_variant_identifiers,
                &hash_blurhashes,
            )
                .transaction(
                    |(
                        hashes,
                        hashes_inverse,
                        hash_identifiers,
                        hash_motion_identifiers,
                        hash_variant_identifiers,
                        hash_blurhashes,
                    )| {
                        if let Some(value) = hashes.remove(&hash)? {
                            hashes_inverse.remove(value)?;
                        }

                        hash_identifiers.remove(&hash)?;
                        hash_motion_identifiers.remove(&hash)?;

                        for key in &variant_keys {
                            hash_variant_identifiers.remove(key)?;
                        }

                        hash_blurhashes.remove(&hash)?;

                        Ok(())
                    },
                )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        match res {
            Ok(()) => Ok(()),
            Err(TransactionError::Abort(e) | TransactionError::Storage(e)) => {
                Err(SledError::from(e).into())
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl VariantRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn claim_variant_processing_rights(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Result<(), NotificationEntry>, RepoError> {
        let key = (hash.clone(), variant.clone());
        let now = time::OffsetDateTime::now_utc();
        let entry = self
            .notifications
            .register_interest(Arc::from(format!("{}{variant}", hash.to_base64())));

        match self.variant_process_map.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(mut occupied_entry) => {
                if occupied_entry
                    .get()
                    .saturating_add(time::Duration::minutes(2))
                    > now
                {
                    return Ok(Err(entry));
                }

                occupied_entry.insert(now);
            }
            dashmap::mapref::entry::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(now);
            }
        }

        if self.variant_identifier(hash, variant).await?.is_some() {
            self.variant_process_map.remove(&key);
            return Ok(Err(entry));
        }

        Ok(Ok(()))
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn variant_heartbeat(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        let key = (hash, variant);
        let now = time::OffsetDateTime::now_utc();

        if let dashmap::mapref::entry::Entry::Occupied(mut occupied_entry) =
            self.variant_process_map.entry(key)
        {
            occupied_entry.insert(now);
        }

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn notify_variant(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        let key = (hash.clone(), variant.clone());
        self.variant_process_map.remove(&key);

        let key = format!("{}{variant}", hash.to_base64());
        self.notifications.notify(&key);

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn relate_variant_identifier(
        &self,
        hash: Hash,
        variant: String,
        identifier: &Arc<str>,
    ) -> Result<Result<(), VariantAlreadyExists>, RepoError> {
        let hash = hash.to_bytes();

        let key = variant_key(&hash, &variant);
        let value = identifier.clone();

        let hash_variant_identifiers = self.hash_variant_identifiers.clone();

        let out = crate::sync::spawn_blocking("sled-io", move || {
            hash_variant_identifiers
                .compare_and_swap(key, Option::<&[u8]>::None, Some(value.as_bytes()))
                .map(|res| res.map_err(|_| VariantAlreadyExists))
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(SledError::from)
        .map_err(RepoError::from)?;

        Ok(out)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn variant_identifier(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<Arc<str>>, RepoError> {
        let hash = hash.to_bytes();

        let key = variant_key(&hash, &variant);

        let opt = b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.get(key)
        );

        Ok(opt.map(try_into_arc_str).transpose()?)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn variants(&self, hash: Hash) -> Result<Vec<(String, Arc<str>)>, RepoError> {
        let hash = hash.to_ivec();

        let vec = b!(
            self.hash_variant_identifiers,
            Ok(hash_variant_identifiers
                .scan_prefix(hash.clone())
                .filter_map(|res| res.ok())
                .filter_map(|(key, ivec)| {
                    let identifier = try_into_arc_str(ivec).ok();

                    let variant = variant_from_key(&hash, &key);
                    if variant.is_none() {
                        tracing::warn!("Skipping a variant: {}", String::from_utf8_lossy(&key));
                    }

                    Some((variant?, identifier?))
                })
                .collect::<Vec<_>>()) as Result<Vec<_>, SledError>
        );

        Ok(vec)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn remove_variant(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        let hash = hash.to_bytes();

        let key = variant_key(&hash, &variant);

        b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.remove(key)
        );

        Ok(())
    }
}

fn hash_alias_key(hash: &IVec, alias: &IVec) -> Vec<u8> {
    let mut v = hash.to_vec();
    v.extend_from_slice(alias);
    v
}

#[async_trait::async_trait(?Send)]
impl AliasRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn create_alias(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
        hash: Hash,
    ) -> Result<Result<(), AliasAlreadyExists>, RepoError> {
        let hash = hash.to_ivec();
        let alias: sled::IVec = alias.to_bytes().into();
        let delete_token: sled::IVec = delete_token.to_bytes().into();

        let aliases = self.aliases.clone();
        let alias_hashes = self.alias_hashes.clone();
        let hash_aliases = self.hash_aliases.clone();
        let alias_delete_tokens = self.alias_delete_tokens.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (&aliases, &alias_hashes, &hash_aliases, &alias_delete_tokens).transaction(
                |(aliases, alias_hashes, hash_aliases, alias_delete_tokens)| {
                    if aliases.get(&alias)?.is_some() {
                        return Ok(Err(AliasAlreadyExists));
                    }

                    aliases.insert(&alias, &alias)?;
                    alias_hashes.insert(&alias, &hash)?;

                    hash_aliases.insert(hash_alias_key(&hash, &alias), &alias)?;
                    alias_delete_tokens.insert(&alias, &delete_token)?;

                    Ok(Ok(()))
                },
            )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        match res {
            Ok(res) => Ok(res),
            Err(TransactionError::Abort(e) | TransactionError::Storage(e)) => {
                Err(SledError::from(e).into())
            }
        }
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn delete_token(&self, alias: &Alias) -> Result<Option<DeleteToken>, RepoError> {
        let key = alias.to_bytes();

        let Some(ivec) = b!(self.alias_delete_tokens, alias_delete_tokens.get(key)) else {
            return Ok(None);
        };

        let Some(token) = DeleteToken::from_slice(&ivec) else {
            return Ok(None);
        };

        Ok(Some(token))
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn hash(&self, alias: &Alias) -> Result<Option<Hash>, RepoError> {
        let key = alias.to_bytes();

        let opt = b!(self.alias_hashes, alias_hashes.get(key));

        Ok(opt.and_then(Hash::from_ivec))
    }

    #[tracing::instrument(skip_all)]
    async fn aliases_for_hash(&self, hash: Hash) -> Result<Vec<Alias>, RepoError> {
        let hash = hash.to_ivec();

        let v = b!(self.hash_aliases, {
            Ok(hash_aliases
                .scan_prefix(hash)
                .values()
                .filter_map(Result::ok)
                .filter_map(|ivec| Alias::from_slice(&ivec))
                .collect::<Vec<_>>()) as Result<_, sled::Error>
        });

        Ok(v)
    }

    #[tracing::instrument(skip(self))]
    async fn cleanup_alias(&self, alias: &Alias) -> Result<(), RepoError> {
        let alias: IVec = alias.to_bytes().into();

        let aliases = self.aliases.clone();
        let alias_hashes = self.alias_hashes.clone();
        let hash_aliases = self.hash_aliases.clone();
        let alias_delete_tokens = self.alias_delete_tokens.clone();

        let res = crate::sync::spawn_blocking("sled-io", move || {
            (&aliases, &alias_hashes, &hash_aliases, &alias_delete_tokens).transaction(
                |(aliases, alias_hashes, hash_aliases, alias_delete_tokens)| {
                    aliases.remove(&alias)?;
                    if let Some(hash) = alias_hashes.remove(&alias)? {
                        hash_aliases.remove(hash_alias_key(&hash, &alias))?;
                    }
                    alias_delete_tokens.remove(&alias)?;
                    Ok(())
                },
            )
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        match res {
            Ok(()) => Ok(()),
            Err(TransactionError::Abort(e)) | Err(TransactionError::Storage(e)) => {
                Err(SledError::from(e).into())
            }
        }
    }
}

impl std::fmt::Debug for SledRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SledRepo").finish()
    }
}

impl From<tokio::task::JoinError> for SledError {
    fn from(_: tokio::task::JoinError) -> Self {
        SledError::Panic
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let hash = crate::repo::Hash::test_value();
        let variant = String::from("some string value");

        let key = super::variant_access_key(&hash.to_bytes(), &variant);

        let (out_hash, out_variant) =
            super::parse_variant_access_key(sled::IVec::from(key)).expect("Parsed bytes");

        assert_eq!(out_hash, hash);
        assert_eq!(out_variant, variant);
    }
}
