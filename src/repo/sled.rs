use crate::{
    details::MaybeHumanDate,
    repo::{
        hash::Hash, Alias, AliasAlreadyExists, AliasRepo, BaseRepo, DeleteToken, Details, FullRepo,
        HashAlreadyExists, HashRepo, Identifier, IdentifierRepo, JobId, MigrationRepo, QueueRepo,
        SettingsRepo, UploadId, UploadRepo, UploadResult,
    },
    serde_str::Serde,
    store::StoreError,
    stream::from_iterator,
};
use futures_util::{Future, Stream};
use sled::{transaction::TransactionError, Db, IVec, Transactional, Tree};
use std::{
    collections::HashMap,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Instant,
};
use tokio::{sync::Notify, task::JoinHandle};
use url::Url;

use super::{AliasAccessRepo, ProxyRepo, RepoError, VariantAccessRepo};

macro_rules! b {
    ($self:ident.$ident:ident, $expr:expr) => {{
        let $ident = $self.$ident.clone();

        let span = tracing::Span::current();

        actix_rt::task::spawn_blocking(move || span.in_scope(|| $expr))
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
    Details(#[from] serde_json::Error),

    #[error("Error formatting timestamp")]
    Format(#[source] time::error::Format),

    #[error("Error parsing variant key")]
    VariantKey(#[from] VariantKeyError),

    #[error("Operation panicked")]
    Panic,

    #[error("Another process updated this value before us")]
    Conflict,
}

#[derive(Clone)]
pub(crate) struct SledRepo {
    healthz_count: Arc<AtomicU64>,
    healthz: Tree,
    settings: Tree,
    identifier_details: Tree,
    hashes: Tree,
    hash_aliases: Tree,
    hash_identifiers: Tree,
    hash_variant_identifiers: Tree,
    hash_motion_identifiers: Tree,
    aliases: Tree,
    alias_hashes: Tree,
    alias_delete_tokens: Tree,
    queue: Tree,
    job_state: Tree,
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
            hash_aliases: db.open_tree("pict-rs-hash-aliases-tree")?,
            hash_identifiers: db.open_tree("pict-rs-hash-identifiers-tree")?,
            hash_variant_identifiers: db.open_tree("pict-rs-hash-variant-identifiers-tree")?,
            hash_motion_identifiers: db.open_tree("pict-rs-hash-motion-identifiers-tree")?,
            aliases: db.open_tree("pict-rs-aliases-tree")?,
            alias_hashes: db.open_tree("pict-rs-alias-hashes-tree")?,
            alias_delete_tokens: db.open_tree("pict-rs-alias-delete-tokens-tree")?,
            queue: db.open_tree("pict-rs-queue-tree")?,
            job_state: db.open_tree("pict-rs-job-state-tree")?,
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

    #[tracing::instrument(level = "debug", skip_all)]
    pub(crate) async fn mark_accessed<I: Identifier + 'static>(&self) -> Result<(), StoreError> {
        use futures_util::StreamExt;

        let mut stream = self.hashes().await;

        while let Some(res) = stream.next().await {
            let hash = res?;

            for (variant, _) in self.variants::<I>(hash.clone()).await? {
                if !self.contains_variant(hash.clone(), variant.clone()).await? {
                    VariantAccessRepo::accessed(self, hash.clone(), variant).await?;
                }
            }
        }

        Ok(())
    }

    #[tracing::instrument(level = "warn", skip_all)]
    pub(crate) async fn export(&self) -> Result<(), RepoError> {
        let path = self
            .export_path
            .join(MaybeHumanDate::HumanDate(time::OffsetDateTime::now_utc()).to_string());

        let export_db = Self::open(path, self.cache_capacity)?;

        let this = self.db.clone();

        actix_rt::task::spawn_blocking(move || {
            let export = this.export();
            export_db.import(export);
        })
        .await
        .map_err(SledError::from)?;

        Ok(())
    }
}

impl BaseRepo for SledRepo {
    type Bytes = IVec;
}

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

        actix_web::web::block(move || {
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

        actix_web::web::block(move || {
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

type IterValue = Option<(sled::Iter, Result<IVec, RepoError>)>;

pub(crate) struct IterStream {
    iter: Option<sled::Iter>,
    next: Option<JoinHandle<IterValue>>,
}

impl futures_util::Stream for IterStream {
    type Item = Result<IVec, RepoError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if let Some(ref mut next) = self.next {
            let res = std::task::ready!(Pin::new(next).poll(cx));

            self.next.take();

            let opt = match res {
                Ok(opt) => opt,
                Err(_) => return std::task::Poll::Ready(Some(Err(RepoError::Canceled))),
            };

            if let Some((iter, res)) = opt {
                self.iter = Some(iter);

                std::task::Poll::Ready(Some(res))
            } else {
                std::task::Poll::Ready(None)
            }
        } else if let Some(mut iter) = self.iter.take() {
            self.next = Some(actix_rt::task::spawn_blocking(move || {
                let opt = iter
                    .next()
                    .map(|res| res.map_err(SledError::from).map_err(RepoError::from));

                opt.map(|res| (iter, res.map(|(_, value)| value)))
            }));
            self.poll_next(cx)
        } else {
            std::task::Poll::Ready(None)
        }
    }
}

pub(crate) struct AliasAccessStream {
    iter: IterStream,
}

pub(crate) struct VariantAccessStream {
    iter: IterStream,
}

impl futures_util::Stream for AliasAccessStream {
    type Item = Result<Alias, RepoError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match std::task::ready!(Pin::new(&mut self.iter).poll_next(cx)) {
            Some(Ok(bytes)) => {
                if let Some(alias) = Alias::from_slice(&bytes) {
                    std::task::Poll::Ready(Some(Ok(alias)))
                } else {
                    self.poll_next(cx)
                }
            }
            Some(Err(e)) => std::task::Poll::Ready(Some(Err(e))),
            None => std::task::Poll::Ready(None),
        }
    }
}

impl futures_util::Stream for VariantAccessStream {
    type Item = Result<(IVec, String), RepoError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match std::task::ready!(Pin::new(&mut self.iter).poll_next(cx)) {
            Some(Ok(bytes)) => std::task::Poll::Ready(Some(
                parse_variant_access_key(bytes)
                    .map_err(SledError::from)
                    .map_err(RepoError::from),
            )),
            Some(Err(e)) => std::task::Poll::Ready(Some(Err(e))),
            None => std::task::Poll::Ready(None),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl AliasAccessRepo for SledRepo {
    type AliasAccessStream = AliasAccessStream;

    #[tracing::instrument(level = "debug", skip(self))]
    async fn accessed(&self, alias: Alias) -> Result<(), RepoError> {
        let now_string = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(SledError::Format)?;

        let alias_access = self.alias_access.clone();
        let inverse_alias_access = self.inverse_alias_access.clone();

        actix_rt::task::spawn_blocking(move || {
            if let Some(old) = alias_access.insert(alias.to_bytes(), now_string.as_bytes())? {
                inverse_alias_access.remove(old)?;
            }
            inverse_alias_access.insert(now_string, alias.to_bytes())?;
            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn older_aliases(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<Self::AliasAccessStream, RepoError> {
        let time_string = timestamp
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(SledError::Format)?;

        let inverse_alias_access = self.inverse_alias_access.clone();

        let iter =
            actix_rt::task::spawn_blocking(move || inverse_alias_access.range(..=time_string))
                .await
                .map_err(|_| RepoError::Canceled)?;

        Ok(AliasAccessStream {
            iter: IterStream {
                iter: Some(iter),
                next: None,
            },
        })
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn remove_access(&self, alias: Alias) -> Result<(), RepoError> {
        let alias_access = self.alias_access.clone();
        let inverse_alias_access = self.inverse_alias_access.clone();

        actix_rt::task::spawn_blocking(move || {
            if let Some(old) = alias_access.remove(alias.to_bytes())? {
                inverse_alias_access.remove(old)?;
            }
            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
    }
}

#[async_trait::async_trait(?Send)]
impl VariantAccessRepo for SledRepo {
    type VariantAccessStream = VariantAccessStream;

    #[tracing::instrument(level = "debug", skip(self))]
    async fn accessed(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        let hash = hash.to_bytes();
        let key = variant_access_key(&hash, &variant);

        let now_string = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(SledError::Format)?;

        let variant_access = self.variant_access.clone();
        let inverse_variant_access = self.inverse_variant_access.clone();

        actix_rt::task::spawn_blocking(move || {
            if let Some(old) = variant_access.insert(&key, now_string.as_bytes())? {
                inverse_variant_access.remove(old)?;
            }
            inverse_variant_access.insert(now_string, key)?;
            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn contains_variant(&self, hash: Hash, variant: String) -> Result<bool, RepoError> {
        let hash = hash.to_bytes();
        let key = variant_access_key(&hash, &variant);

        let timestamp = b!(self.variant_access, variant_access.get(key));

        Ok(timestamp.is_some())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn older_variants(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<Self::VariantAccessStream, RepoError> {
        let time_string = timestamp
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(SledError::Format)?;

        let inverse_variant_access = self.inverse_variant_access.clone();

        let iter =
            actix_rt::task::spawn_blocking(move || inverse_variant_access.range(..=time_string))
                .await
                .map_err(|_| RepoError::Canceled)?;

        Ok(VariantAccessStream {
            iter: IterStream {
                iter: Some(iter),
                next: None,
            },
        })
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn remove_access(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        let hash = hash.to_bytes();
        let key = variant_access_key(&hash, &variant);

        let variant_access = self.variant_access.clone();
        let inverse_variant_access = self.inverse_variant_access.clone();

        actix_rt::task::spawn_blocking(move || {
            if let Some(old) = variant_access.remove(key)? {
                inverse_variant_access.remove(old)?;
            }
            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
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
    },
}

impl From<UploadResult> for InnerUploadResult {
    fn from(u: UploadResult) -> Self {
        match u {
            UploadResult::Success { alias, token } => InnerUploadResult::Success {
                alias: Serde::new(alias),
                token: Serde::new(token),
            },
            UploadResult::Failure { message } => InnerUploadResult::Failure { message },
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
            InnerUploadResult::Failure { message } => UploadResult::Failure { message },
        }
    }
}

struct PushMetricsGuard {
    queue: &'static str,
    armed: bool,
}

struct PopMetricsGuard {
    queue: &'static str,
    start: Instant,
    armed: bool,
}

impl PushMetricsGuard {
    fn guard(queue: &'static str) -> Self {
        Self { queue, armed: true }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl PopMetricsGuard {
    fn guard(queue: &'static str) -> Self {
        Self {
            queue,
            start: Instant::now(),
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PushMetricsGuard {
    fn drop(&mut self) {
        metrics::increment_counter!("pict-rs.queue.push", "completed" => (!self.armed).to_string(), "queue" => self.queue);
    }
}

impl Drop for PopMetricsGuard {
    fn drop(&mut self) {
        metrics::histogram!("pict-rs.queue.pop.duration", self.start.elapsed().as_secs_f64(), "completed" => (!self.armed).to_string(), "queue" => self.queue);
        metrics::increment_counter!("pict-rs.queue.pop", "completed" => (!self.armed).to_string(), "queue" => self.queue);
    }
}

#[async_trait::async_trait(?Send)]
impl UploadRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn create(&self, upload_id: UploadId) -> Result<(), RepoError> {
        b!(self.uploads, uploads.insert(upload_id.as_bytes(), b"1"));
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, RepoError> {
        let mut subscriber = self.uploads.watch_prefix(upload_id.as_bytes());

        let bytes = upload_id.as_bytes().to_vec();
        let opt = b!(self.uploads, uploads.get(bytes));

        if let Some(bytes) = opt {
            if bytes != b"1" {
                let result: InnerUploadResult =
                    serde_json::from_slice(&bytes).map_err(SledError::from)?;
                return Ok(result.into());
            }
        } else {
            return Err(RepoError::AlreadyClaimed);
        }

        while let Some(event) = (&mut subscriber).await {
            match event {
                sled::Event::Remove { .. } => {
                    return Err(RepoError::AlreadyClaimed);
                }
                sled::Event::Insert { value, .. } => {
                    if value != b"1" {
                        let result: InnerUploadResult =
                            serde_json::from_slice(&value).map_err(SledError::from)?;
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
    async fn complete(&self, upload_id: UploadId, result: UploadResult) -> Result<(), RepoError> {
        let result: InnerUploadResult = result.into();
        let result = serde_json::to_vec(&result).map_err(SledError::from)?;

        b!(self.uploads, uploads.insert(upload_id.as_bytes(), result));

        Ok(())
    }
}

enum JobState {
    Pending,
    Running([u8; 8]),
}

impl JobState {
    const fn pending() -> Self {
        Self::Pending
    }

    fn running() -> Self {
        Self::Running(
            time::OffsetDateTime::now_utc()
                .unix_timestamp()
                .to_be_bytes(),
        )
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

#[async_trait::async_trait(?Send)]
impl QueueRepo for SledRepo {
    #[tracing::instrument(skip(self, job), fields(job = %String::from_utf8_lossy(&job)))]
    async fn push(&self, queue_name: &'static str, job: Self::Bytes) -> Result<JobId, RepoError> {
        let metrics_guard = PushMetricsGuard::guard(queue_name);

        let id = JobId::gen();
        let key = job_key(queue_name, id);

        let queue = self.queue.clone();
        let job_state = self.job_state.clone();

        let res = actix_rt::task::spawn_blocking(move || {
            (&queue, &job_state).transaction(|(queue, job_state)| {
                let state = JobState::pending();

                queue.insert(&key[..], &job)?;
                job_state.insert(&key[..], state.as_bytes())?;

                Ok(())
            })
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        if let Err(TransactionError::Abort(e) | TransactionError::Storage(e)) = res {
            return Err(RepoError::from(SledError::from(e)));
        }

        if let Some(notifier) = self.queue_notifier.read().unwrap().get(&queue_name) {
            notifier.notify_one();
            metrics_guard.disarm();
            return Ok(id);
        }

        self.queue_notifier
            .write()
            .unwrap()
            .entry(queue_name)
            .or_insert_with(|| Arc::new(Notify::new()))
            .notify_one();

        metrics_guard.disarm();

        Ok(id)
    }

    #[tracing::instrument(skip(self))]
    async fn pop(&self, queue_name: &'static str) -> Result<(JobId, Self::Bytes), RepoError> {
        let metrics_guard = PopMetricsGuard::guard(queue_name);

        let now = time::OffsetDateTime::now_utc();

        loop {
            let queue = self.queue.clone();
            let job_state = self.job_state.clone();

            let opt = actix_rt::task::spawn_blocking(move || {
                // Job IDs are generated with Uuid version 7 - defining their first bits as a
                // timestamp. Scanning a prefix should give us jobs in the order they were queued.
                for res in job_state.scan_prefix(queue_name) {
                    let (key, value) = res?;

                    if value.len() == 8 {
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

                    let state = JobState::running();

                    match job_state.compare_and_swap(&key, Some(value), Some(state.as_bytes())) {
                        Ok(_) => {
                            // acquired job
                        }
                        Err(_) => {
                            // someone else acquired job
                            continue;
                        }
                    }

                    let id_bytes = &key[queue_name.len()..];

                    let id_bytes: [u8; 16] = id_bytes.try_into().expect("Key length");

                    let job_id = JobId::from_bytes(id_bytes);

                    let opt = queue.get(&key)?.map(|job_bytes| (job_id, job_bytes));

                    return Ok(opt) as Result<Option<(JobId, Self::Bytes)>, SledError>;
                }

                Ok(None)
            })
            .await
            .map_err(|_| RepoError::Canceled)??;

            if let Some(tup) = opt {
                metrics_guard.disarm();
                return Ok(tup);
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
                let entry = guard
                    .entry(queue_name)
                    .or_insert_with(|| Arc::new(Notify::new()));
                Arc::clone(entry)
            };

            notify.notified().await
        }
    }

    #[tracing::instrument(skip(self))]
    async fn heartbeat(&self, queue_name: &'static str, job_id: JobId) -> Result<(), RepoError> {
        let key = job_key(queue_name, job_id);

        let job_state = self.job_state.clone();

        actix_rt::task::spawn_blocking(move || {
            if let Some(state) = job_state.get(&key)? {
                let new_state = JobState::running();

                match job_state.compare_and_swap(&key, Some(state), Some(new_state.as_bytes()))? {
                    Ok(_) => Ok(()),
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

    #[tracing::instrument(skip(self))]
    async fn complete_job(&self, queue_name: &'static str, job_id: JobId) -> Result<(), RepoError> {
        let key = job_key(queue_name, job_id);

        let queue = self.queue.clone();
        let job_state = self.job_state.clone();

        let res = actix_rt::task::spawn_blocking(move || {
            (&queue, &job_state).transaction(|(queue, job_state)| {
                queue.remove(&key[..])?;
                job_state.remove(&key[..])?;
                Ok(())
            })
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
impl SettingsRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(value))]
    async fn set(&self, key: &'static str, value: Self::Bytes) -> Result<(), RepoError> {
        b!(self.settings, settings.insert(key, value));

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn get(&self, key: &'static str) -> Result<Option<Self::Bytes>, RepoError> {
        let opt = b!(self.settings, settings.get(key));

        Ok(opt)
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
}

fn parse_variant_access_key(bytes: IVec) -> Result<(IVec, String), VariantKeyError> {
    if bytes.len() < 8 {
        return Err(VariantKeyError::TooShort);
    }

    let hash_len = u64::from_be_bytes(bytes[..8].try_into().expect("Verified length"));
    let hash_len: usize = usize::try_from(hash_len).expect("Length is reasonable");

    if (hash_len + 8) > bytes.len() {
        return Err(VariantKeyError::InvalidLength);
    }

    let hash = bytes.subslice(8, hash_len);

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
impl IdentifierRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self, identifier), fields(identifier = identifier.string_repr()))]
    async fn relate_details<I: Identifier>(
        &self,
        identifier: &I,
        details: &Details,
    ) -> Result<(), StoreError> {
        let key = identifier.to_bytes()?;
        let details = serde_json::to_vec(&details)
            .map_err(SledError::from)
            .map_err(RepoError::from)?;

        b!(
            self.identifier_details,
            identifier_details.insert(key, details)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self, identifier), fields(identifier = identifier.string_repr()))]
    async fn details<I: Identifier>(&self, identifier: &I) -> Result<Option<Details>, StoreError> {
        let key = identifier.to_bytes()?;

        let opt = b!(self.identifier_details, identifier_details.get(key));

        opt.map(|ivec| serde_json::from_slice(&ivec))
            .transpose()
            .map_err(SledError::from)
            .map_err(RepoError::from)
            .map_err(StoreError::from)
    }

    #[tracing::instrument(level = "trace", skip(self, identifier), fields(identifier = identifier.string_repr()))]
    async fn cleanup<I: Identifier>(&self, identifier: &I) -> Result<(), StoreError> {
        let key = identifier.to_bytes()?;

        b!(self.identifier_details, identifier_details.remove(key));

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl MigrationRepo for SledRepo {
    async fn is_continuing_migration(&self) -> Result<bool, RepoError> {
        Ok(!self.migration_identifiers.is_empty())
    }

    async fn mark_migrated<I1: Identifier, I2: Identifier>(
        &self,
        old_identifier: &I1,
        new_identifier: &I2,
    ) -> Result<(), StoreError> {
        let key = new_identifier.to_bytes()?;
        let value = old_identifier.to_bytes()?;

        b!(
            self.migration_identifiers,
            migration_identifiers.insert(key, value)
        );

        Ok(())
    }

    async fn is_migrated<I: Identifier>(&self, identifier: &I) -> Result<bool, StoreError> {
        let key = identifier.to_bytes()?;

        Ok(b!(self.migration_identifiers, migration_identifiers.get(key)).is_some())
    }

    async fn clear(&self) -> Result<(), RepoError> {
        b!(self.migration_identifiers, migration_identifiers.clear());

        Ok(())
    }
}

type StreamItem = Result<Hash, RepoError>;
type LocalBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

#[async_trait::async_trait(?Send)]
impl HashRepo for SledRepo {
    type Stream = LocalBoxStream<'static, StreamItem>;

    async fn size(&self) -> Result<u64, RepoError> {
        Ok(b!(
            self.hashes,
            Ok(u64::try_from(hashes.len()).expect("Length is reasonable"))
                as Result<u64, SledError>
        ))
    }

    async fn hashes(&self) -> Self::Stream {
        let iter = self.hashes.iter().keys().filter_map(|res| {
            res.map_err(SledError::from)
                .map_err(RepoError::from)
                .map(Hash::from_ivec)
                .transpose()
        });

        Box::pin(from_iterator(iter, 8))
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn create<I: Identifier>(
        &self,
        hash: Hash,
        identifier: &I,
    ) -> Result<Result<(), HashAlreadyExists>, StoreError> {
        let identifier: sled::IVec = identifier.to_bytes()?.into();

        let hashes = self.hashes.clone();
        let hash_identifiers = self.hash_identifiers.clone();

        let hash = hash.to_ivec();

        let res = actix_web::web::block(move || {
            (&hashes, &hash_identifiers).transaction(|(hashes, hash_identifiers)| {
                if hashes.get(hash.clone())?.is_some() {
                    return Ok(Err(HashAlreadyExists));
                }

                hashes.insert(hash.clone(), hash.clone())?;
                hash_identifiers.insert(hash.clone(), &identifier)?;

                Ok(Ok(()))
            })
        })
        .await
        .map_err(|_| RepoError::Canceled)?;

        match res {
            Ok(res) => Ok(res),
            Err(TransactionError::Abort(e) | TransactionError::Storage(e)) => {
                Err(StoreError::from(RepoError::from(SledError::from(e))))
            }
        }
    }

    async fn update_identifier<I: Identifier>(
        &self,
        hash: Hash,
        identifier: &I,
    ) -> Result<(), StoreError> {
        let identifier = identifier.to_bytes()?;

        let hash = hash.to_ivec();

        b!(
            self.hash_identifiers,
            hash_identifiers.insert(hash, identifier)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn identifier<I: Identifier + 'static>(
        &self,
        hash: Hash,
    ) -> Result<Option<I>, StoreError> {
        let hash = hash.to_ivec();

        let Some(ivec) = b!(self.hash_identifiers, hash_identifiers.get(hash)) else {
            return Ok(None);
        };

        Ok(Some(I::from_bytes(ivec.to_vec())?))
    }

    #[tracing::instrument(level = "trace", skip(self, identifier), fields(identifier = identifier.string_repr()))]
    async fn relate_variant_identifier<I: Identifier>(
        &self,
        hash: Hash,
        variant: String,
        identifier: &I,
    ) -> Result<(), StoreError> {
        let hash = hash.to_bytes();

        let key = variant_key(&hash, &variant);
        let value = identifier.to_bytes()?;

        b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.insert(key, value)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn variant_identifier<I: Identifier + 'static>(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<I>, StoreError> {
        let hash = hash.to_bytes();

        let key = variant_key(&hash, &variant);

        let opt = b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.get(key)
        );

        opt.map(|ivec| I::from_bytes(ivec.to_vec())).transpose()
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn variants<I: Identifier + 'static>(
        &self,
        hash: Hash,
    ) -> Result<Vec<(String, I)>, StoreError> {
        let hash = hash.to_ivec();

        let vec = b!(
            self.hash_variant_identifiers,
            Ok(hash_variant_identifiers
                .scan_prefix(hash.clone())
                .filter_map(|res| res.ok())
                .filter_map(|(key, ivec)| {
                    let identifier = I::from_bytes(ivec.to_vec()).ok();
                    if identifier.is_none() {
                        tracing::warn!(
                            "Skipping an identifier: {}",
                            String::from_utf8_lossy(&ivec)
                        );
                    }

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

    #[tracing::instrument(level = "trace", skip(self, identifier), fields(identifier = identifier.string_repr()))]
    async fn relate_motion_identifier<I: Identifier>(
        &self,
        hash: Hash,
        identifier: &I,
    ) -> Result<(), StoreError> {
        let hash = hash.to_ivec();
        let bytes = identifier.to_bytes()?;

        b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.insert(hash, bytes)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn motion_identifier<I: Identifier + 'static>(
        &self,
        hash: Hash,
    ) -> Result<Option<I>, StoreError> {
        let hash = hash.to_ivec();

        let opt = b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.get(hash)
        );

        opt.map(|ivec| I::from_bytes(ivec.to_vec())).transpose()
    }

    #[tracing::instrument(skip(self))]
    async fn cleanup(&self, hash: Hash) -> Result<(), RepoError> {
        let hash = hash.to_ivec();

        let hashes = self.hashes.clone();
        let hash_identifiers = self.hash_identifiers.clone();
        let hash_motion_identifiers = self.hash_motion_identifiers.clone();
        let hash_variant_identifiers = self.hash_variant_identifiers.clone();

        let hash2 = hash.clone();
        let variant_keys = b!(self.hash_variant_identifiers, {
            let v = hash_variant_identifiers
                .scan_prefix(hash2)
                .keys()
                .filter_map(Result::ok)
                .collect::<Vec<_>>();

            Ok(v) as Result<Vec<_>, SledError>
        });

        let res = actix_web::web::block(move || {
            (
                &hashes,
                &hash_identifiers,
                &hash_motion_identifiers,
                &hash_variant_identifiers,
            )
                .transaction(
                    |(
                        hashes,
                        hash_identifiers,
                        hash_motion_identifiers,
                        hash_variant_identifiers,
                    )| {
                        hashes.remove(&hash)?;
                        hash_identifiers.remove(&hash)?;
                        hash_motion_identifiers.remove(&hash)?;

                        for key in &variant_keys {
                            hash_variant_identifiers.remove(key)?;
                        }

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

fn hash_alias_key(hash: &IVec, alias: &IVec) -> Vec<u8> {
    let mut v = hash.to_vec();
    v.extend_from_slice(alias);
    v
}

#[async_trait::async_trait(?Send)]
impl AliasRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn create(
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

        let res = actix_web::web::block(move || {
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
    async fn for_hash(&self, hash: Hash) -> Result<Vec<Alias>, RepoError> {
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
    async fn cleanup(&self, alias: &Alias) -> Result<(), RepoError> {
        let alias: IVec = alias.to_bytes().into();

        let aliases = self.aliases.clone();
        let alias_hashes = self.alias_hashes.clone();
        let hash_aliases = self.hash_aliases.clone();
        let alias_delete_tokens = self.alias_delete_tokens.clone();

        let res = actix_web::web::block(move || {
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

impl From<actix_rt::task::JoinError> for SledError {
    fn from(_: actix_rt::task::JoinError) -> Self {
        SledError::Panic
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let hash = sled::IVec::from(b"some hash value");
        let variant = String::from("some string value");

        let key = super::variant_access_key(&hash, &variant);

        let (out_hash, out_variant) =
            super::parse_variant_access_key(sled::IVec::from(key)).expect("Parsed bytes");

        assert_eq!(out_hash, hash);
        assert_eq!(out_variant, variant);
    }
}
