use crate::{
    details::MaybeHumanDate,
    repo::{
        Alias, AliasAlreadyExists, AliasRepo, AlreadyExists, BaseRepo, DeleteToken, Details,
        FullRepo, HashAlreadyExists, HashRepo, Identifier, IdentifierRepo, MigrationRepo,
        QueueRepo, SettingsRepo, UploadId, UploadRepo, UploadResult,
    },
    serde_str::Serde,
    store::StoreError,
    stream::from_iterator,
};
use futures_util::{Future, Stream};
use sled::{CompareAndSwapError, Db, IVec, Tree};
use std::{
    collections::HashMap,
    marker::PhantomData,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};
use tokio::{sync::Notify, task::JoinHandle};

use super::{AliasAccessRepo, IdentifierAccessRepo, RepoError};

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

    #[error("Operation panicked")]
    Panic,
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
    alias_access: Tree,
    inverse_alias_access: Tree,
    identifier_access: Tree,
    inverse_identifier_access: Tree,
    in_progress_queue: Tree,
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
            alias_access: db.open_tree("pict-rs-alias-access-tree")?,
            inverse_alias_access: db.open_tree("pict-rs-inverse-alias-access-tree")?,
            identifier_access: db.open_tree("pict-rs-identifier-access-tree")?,
            inverse_identifier_access: db.open_tree("pict-rs-inverse-identifier-access-tree")?,
            in_progress_queue: db.open_tree("pict-rs-in-progress-queue-tree")?,
            queue_notifier: Arc::new(RwLock::new(HashMap::new())),
            uploads: db.open_tree("pict-rs-uploads-tree")?,
            migration_identifiers: db.open_tree("pict-rs-migration-identifiers-tree")?,
            cache_capacity,
            export_path,
            db,
        })
    }

    fn open(mut path: PathBuf, cache_capacity: u64) -> Result<Db, SledError> {
        path.push("v0.4.0-alpha.1");

        let db = ::sled::Config::new()
            .cache_capacity(cache_capacity)
            .path(path)
            .open()?;

        Ok(db)
    }

    #[tracing::instrument(level = "warn")]
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

type IterValue = Option<(sled::Iter, Result<sled::IVec, RepoError>)>;

pub(crate) struct IterStream {
    iter: Option<sled::Iter>,
    next: Option<JoinHandle<IterValue>>,
}

impl futures_util::Stream for IterStream {
    type Item = Result<sled::IVec, RepoError>;

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
            self.next = Some(tokio::task::spawn_blocking(move || {
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

pub(crate) struct IdentifierAccessStream<I> {
    iter: IterStream,
    identifier: PhantomData<fn() -> I>,
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

impl<I> futures_util::Stream for IdentifierAccessStream<I>
where
    I: Identifier,
{
    type Item = Result<I, StoreError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match std::task::ready!(Pin::new(&mut self.iter).poll_next(cx)) {
            Some(Ok(bytes)) => std::task::Poll::Ready(Some(I::from_bytes(bytes.to_vec()))),
            Some(Err(e)) => std::task::Poll::Ready(Some(Err(e.into()))),
            None => std::task::Poll::Ready(None),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl AliasAccessRepo for SledRepo {
    type AliasAccessStream = AliasAccessStream;

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

    async fn remove(&self, alias: Alias) -> Result<(), RepoError> {
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
impl IdentifierAccessRepo for SledRepo {
    type IdentifierAccessStream<I> = IdentifierAccessStream<I> where I: Identifier;

    async fn accessed<I: Identifier>(&self, identifier: I) -> Result<(), StoreError> {
        let now_string = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(SledError::Format)
            .map_err(RepoError::from)?;

        let identifier_access = self.identifier_access.clone();
        let inverse_identifier_access = self.inverse_identifier_access.clone();

        let identifier = identifier.to_bytes()?;

        actix_rt::task::spawn_blocking(move || {
            if let Some(old) =
                identifier_access.insert(identifier.clone(), now_string.as_bytes())?
            {
                inverse_identifier_access.remove(old)?;
            }
            inverse_identifier_access.insert(now_string, identifier)?;
            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
        .map_err(StoreError::from)
    }

    async fn older_identifiers<I: Identifier>(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<Self::IdentifierAccessStream<I>, RepoError> {
        let time_string = timestamp
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(SledError::Format)?;

        let inverse_identifier_access = self.inverse_identifier_access.clone();

        let iter =
            actix_rt::task::spawn_blocking(move || inverse_identifier_access.range(..=time_string))
                .await
                .map_err(|_| RepoError::Canceled)?;

        Ok(IdentifierAccessStream {
            iter: IterStream {
                iter: Some(iter),
                next: None,
            },
            identifier: PhantomData,
        })
    }

    async fn remove<I: Identifier>(&self, identifier: I) -> Result<(), StoreError> {
        let identifier_access = self.identifier_access.clone();
        let inverse_identifier_access = self.inverse_identifier_access.clone();

        let identifier = identifier.to_bytes()?;

        actix_rt::task::spawn_blocking(move || {
            if let Some(old) = identifier_access.remove(identifier)? {
                inverse_identifier_access.remove(old)?;
            }
            Ok(()) as Result<(), SledError>
        })
        .await
        .map_err(|_| RepoError::Canceled)?
        .map_err(RepoError::from)
        .map_err(StoreError::from)
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

#[async_trait::async_trait(?Send)]
impl QueueRepo for SledRepo {
    #[tracing::instrument(skip_all, fields(worker_id = %String::from_utf8_lossy(&worker_prefix)))]
    async fn requeue_in_progress(&self, worker_prefix: Vec<u8>) -> Result<(), RepoError> {
        let vec: Vec<(String, IVec)> = b!(self.in_progress_queue, {
            let vec = in_progress_queue
                .scan_prefix(worker_prefix)
                .values()
                .filter_map(Result::ok)
                .filter_map(|ivec| {
                    let index = ivec.as_ref().iter().enumerate().find_map(|(index, byte)| {
                        if *byte == 0 {
                            Some(index)
                        } else {
                            None
                        }
                    })?;

                    let (queue, job) = ivec.split_at(index);
                    if queue.is_empty() || job.len() <= 1 {
                        return None;
                    }
                    let job = &job[1..];

                    Some((String::from_utf8_lossy(queue).to_string(), IVec::from(job)))
                })
                .collect::<Vec<(String, IVec)>>();

            Ok(vec) as Result<_, SledError>
        });

        let db = self.db.clone();
        b!(self.queue, {
            for (queue_name, job) in vec {
                let id = db.generate_id()?;
                let mut key = queue_name.as_bytes().to_vec();
                key.extend(id.to_be_bytes());

                queue.insert(key, job)?;
            }

            Ok(()) as Result<(), SledError>
        });

        Ok(())
    }

    #[tracing::instrument(skip(self, job), fields(job = %String::from_utf8_lossy(&job)))]
    async fn push(&self, queue_name: &'static str, job: Self::Bytes) -> Result<(), RepoError> {
        let id = self.db.generate_id().map_err(SledError::from)?;
        let mut key = queue_name.as_bytes().to_vec();
        key.extend(id.to_be_bytes());

        b!(self.queue, queue.insert(key, job));

        if let Some(notifier) = self.queue_notifier.read().unwrap().get(&queue_name) {
            notifier.notify_one();
            return Ok(());
        }

        self.queue_notifier
            .write()
            .unwrap()
            .entry(queue_name)
            .or_insert_with(|| Arc::new(Notify::new()))
            .notify_one();

        Ok(())
    }

    #[tracing::instrument(skip(self, worker_id), fields(worker_id = %String::from_utf8_lossy(&worker_id)))]
    async fn pop(
        &self,
        queue_name: &'static str,
        worker_id: Vec<u8>,
    ) -> Result<Self::Bytes, RepoError> {
        loop {
            let in_progress_queue = self.in_progress_queue.clone();

            let worker_id = worker_id.clone();
            let job = b!(self.queue, {
                in_progress_queue.remove(&worker_id)?;
                in_progress_queue.flush()?;

                while let Some((key, job)) = queue
                    .scan_prefix(queue_name.as_bytes())
                    .find_map(Result::ok)
                {
                    let mut in_progress_value = queue_name.as_bytes().to_vec();
                    in_progress_value.push(0);
                    in_progress_value.extend_from_slice(&job);

                    in_progress_queue.insert(&worker_id, in_progress_value)?;

                    if queue.remove(key)?.is_some() {
                        return Ok(Some(job));
                    }

                    in_progress_queue.remove(&worker_id)?;
                }

                Ok(None) as Result<_, SledError>
            });

            if let Some(job) = job {
                return Ok(job);
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

type StreamItem = Result<IVec, RepoError>;
type LocalBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

fn hash_alias_key(hash: &IVec, alias: &Alias) -> Vec<u8> {
    let mut v = hash.to_vec();
    v.append(&mut alias.to_bytes());
    v
}

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
        let iter = self
            .hashes
            .iter()
            .keys()
            .map(|res| res.map_err(SledError::from).map_err(RepoError::from));

        Box::pin(from_iterator(iter, 8))
    }

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn create(&self, hash: Self::Bytes) -> Result<Result<(), HashAlreadyExists>, RepoError> {
        let res = b!(self.hashes, {
            let hash2 = hash.clone();
            hashes.compare_and_swap(hash, None as Option<Self::Bytes>, Some(hash2))
        });

        Ok(res.map_err(|_| HashAlreadyExists))
    }

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn relate_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), RepoError> {
        let key = hash_alias_key(&hash, alias);
        let value = alias.to_bytes();

        b!(self.hash_aliases, hash_aliases.insert(key, value));

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn remove_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), RepoError> {
        let key = hash_alias_key(&hash, alias);

        b!(self.hash_aliases, hash_aliases.remove(key));

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn aliases(&self, hash: Self::Bytes) -> Result<Vec<Alias>, RepoError> {
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

    #[tracing::instrument(level = "trace", skip(self, hash, identifier), fields(hash = hex::encode(&hash), identifier = identifier.string_repr()))]
    async fn relate_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), StoreError> {
        let bytes = identifier.to_bytes()?;

        b!(self.hash_identifiers, hash_identifiers.insert(hash, bytes));

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, StoreError> {
        let Some(ivec) = b!(self.hash_identifiers, hash_identifiers.get(hash)) else {
            return Ok(None);
        };

        Ok(Some(I::from_bytes(ivec.to_vec())?))
    }

    #[tracing::instrument(level = "trace", skip(self, hash, identifier), fields(hash = hex::encode(&hash), identifier = identifier.string_repr()))]
    async fn relate_variant_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        variant: String,
        identifier: &I,
    ) -> Result<(), StoreError> {
        let key = variant_key(&hash, &variant);
        let value = identifier.to_bytes()?;

        b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.insert(key, value)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn variant_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
        variant: String,
    ) -> Result<Option<I>, StoreError> {
        let key = variant_key(&hash, &variant);

        let opt = b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.get(key)
        );

        opt.map(|ivec| I::from_bytes(ivec.to_vec())).transpose()
    }

    #[tracing::instrument(skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn variants<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Vec<(String, I)>, StoreError> {
        let vec = b!(
            self.hash_variant_identifiers,
            Ok(hash_variant_identifiers
                .scan_prefix(&hash)
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

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn remove_variant(&self, hash: Self::Bytes, variant: String) -> Result<(), RepoError> {
        let key = variant_key(&hash, &variant);

        b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.remove(key)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self, hash, identifier), fields(hash = hex::encode(&hash), identifier = identifier.string_repr()))]
    async fn relate_motion_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), StoreError> {
        let bytes = identifier.to_bytes()?;

        b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.insert(hash, bytes)
        );

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn motion_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, StoreError> {
        let opt = b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.get(hash)
        );

        opt.map(|ivec| I::from_bytes(ivec.to_vec())).transpose()
    }

    #[tracing::instrument(skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn cleanup(&self, hash: Self::Bytes) -> Result<(), RepoError> {
        let hash2 = hash.clone();
        b!(self.hashes, hashes.remove(hash2));

        let hash2 = hash.clone();
        b!(self.hash_identifiers, hash_identifiers.remove(hash2));

        let hash2 = hash.clone();
        b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.remove(hash2)
        );

        let aliases = self.aliases(hash.clone()).await?;
        let hash2 = hash.clone();
        b!(self.hash_aliases, {
            for alias in aliases {
                let key = hash_alias_key(&hash2, &alias);

                let _ = hash_aliases.remove(key);
            }
            Ok(()) as Result<(), SledError>
        });

        let variant_keys = b!(self.hash_variant_identifiers, {
            let v = hash_variant_identifiers
                .scan_prefix(hash)
                .keys()
                .filter_map(Result::ok)
                .collect::<Vec<_>>();

            Ok(v) as Result<Vec<_>, SledError>
        });
        b!(self.hash_variant_identifiers, {
            for key in variant_keys {
                let _ = hash_variant_identifiers.remove(key);
            }
            Ok(()) as Result<(), SledError>
        });

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl AliasRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn create(&self, alias: &Alias) -> Result<Result<(), AliasAlreadyExists>, RepoError> {
        let bytes = alias.to_bytes();
        let bytes2 = bytes.clone();

        let res = b!(
            self.aliases,
            aliases.compare_and_swap(bytes, None as Option<Self::Bytes>, Some(bytes2))
        );

        Ok(res.map_err(|_| AliasAlreadyExists))
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn relate_delete_token(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
    ) -> Result<Result<(), AlreadyExists>, RepoError> {
        let key = alias.to_bytes();
        let token = delete_token.to_bytes();

        let res = b!(self.alias_delete_tokens, {
            let mut prev: Option<Self::Bytes> = None;

            loop {
                let key = key.clone();
                let token = token.clone();

                let res = alias_delete_tokens.compare_and_swap(key, prev, Some(token))?;

                match res {
                    Ok(()) => return Ok(Ok(())) as Result<_, SledError>,
                    Err(CompareAndSwapError {
                        current: Some(token),
                        ..
                    }) => {
                        if let Some(token) = DeleteToken::from_slice(&token) {
                            return Ok(Err(AlreadyExists(token)));
                        } else {
                            prev = Some(token);
                        };
                    }
                    Err(CompareAndSwapError { current: None, .. }) => {
                        prev = None;
                    }
                }
            }
        });

        Ok(res)
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

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn relate_hash(&self, alias: &Alias, hash: Self::Bytes) -> Result<(), RepoError> {
        let key = alias.to_bytes();

        b!(self.alias_hashes, alias_hashes.insert(key, hash));

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn hash(&self, alias: &Alias) -> Result<Option<Self::Bytes>, RepoError> {
        let key = alias.to_bytes();

        let opt = b!(self.alias_hashes, alias_hashes.get(key));

        Ok(opt)
    }

    #[tracing::instrument(skip(self))]
    async fn cleanup(&self, alias: &Alias) -> Result<(), RepoError> {
        let key = alias.to_bytes();

        let key2 = key.clone();
        b!(self.aliases, aliases.remove(key2));

        let key2 = key.clone();
        b!(self.alias_delete_tokens, alias_delete_tokens.remove(key2));

        b!(self.alias_hashes, alias_hashes.remove(key));

        Ok(())
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
