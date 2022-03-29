use crate::{
    error::Error,
    repo::{
        Alias, AliasRepo, AlreadyExists, DeleteToken, Details, HashRepo, Identifier,
        IdentifierRepo, QueueRepo, SettingsRepo,
    },
    stream::from_iterator,
};
use futures_util::Stream;
use sled::{Db, IVec, Tree};
use std::{pin::Pin, sync::Arc};
use tokio::sync::Notify;

use super::BaseRepo;

macro_rules! b {
    ($self:ident.$ident:ident, $expr:expr) => {{
        let $ident = $self.$ident.clone();

        actix_rt::task::spawn_blocking(move || $expr)
            .await
            .map_err(SledError::from)??
    }};
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SledError {
    #[error("Error in database")]
    Sled(#[from] sled::Error),

    #[error("Invalid details json")]
    Details(#[from] serde_json::Error),

    #[error("Required field was not present")]
    Missing,

    #[error("Operation panicked")]
    Panic,
}

#[derive(Clone)]
pub(crate) struct SledRepo {
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
    in_progress_queue: Tree,
    queue_notifier: Arc<Notify>,
    db: Db,
}

impl SledRepo {
    pub(crate) fn new(db: Db) -> Result<Self, SledError> {
        Ok(SledRepo {
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
            in_progress_queue: db.open_tree("pict-rs-in-progress-queue-tree")?,
            queue_notifier: Arc::new(Notify::new()),
            db,
        })
    }
}

impl BaseRepo for SledRepo {
    type Bytes = IVec;
}

#[async_trait::async_trait(?Send)]
impl QueueRepo for SledRepo {
    async fn in_progress(&self, worker_id: Vec<u8>) -> Result<Option<Self::Bytes>, Error> {
        let opt = b!(self.in_progress_queue, in_progress_queue.get(worker_id));

        Ok(opt)
    }

    async fn push(&self, job: Self::Bytes) -> Result<(), Error> {
        let id = self.db.generate_id()?;
        b!(self.queue, queue.insert(id.to_be_bytes(), job));
        self.queue_notifier.notify_one();
        Ok(())
    }

    async fn pop(&self, worker_id: Vec<u8>) -> Result<Self::Bytes, Error> {
        let notify = Arc::clone(&self.queue_notifier);

        loop {
            let in_progress_queue = self.in_progress_queue.clone();

            let worker_id = worker_id.clone();
            let job = b!(self.queue, {
                in_progress_queue.remove(&worker_id)?;

                while let Some((key, job)) = queue.iter().find_map(Result::ok) {
                    in_progress_queue.insert(&worker_id, &job)?;

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

            notify.notified().await
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SettingsRepo for SledRepo {
    #[tracing::instrument(skip(value))]
    async fn set(&self, key: &'static [u8], value: Self::Bytes) -> Result<(), Error> {
        b!(self.settings, settings.insert(key, value));

        Ok(())
    }

    #[tracing::instrument]
    async fn get(&self, key: &'static [u8]) -> Result<Option<Self::Bytes>, Error> {
        let opt = b!(self.settings, settings.get(key));

        Ok(opt)
    }

    #[tracing::instrument]
    async fn remove(&self, key: &'static [u8]) -> Result<(), Error> {
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
    #[tracing::instrument]
    async fn relate_details<I: Identifier>(
        &self,
        identifier: &I,
        details: &Details,
    ) -> Result<(), Error> {
        let key = identifier.to_bytes()?;
        let details = serde_json::to_vec(&details)?;

        b!(
            self.identifier_details,
            identifier_details.insert(key, details)
        );

        Ok(())
    }

    #[tracing::instrument]
    async fn details<I: Identifier>(&self, identifier: &I) -> Result<Option<Details>, Error> {
        let key = identifier.to_bytes()?;

        let opt = b!(self.identifier_details, identifier_details.get(key));

        if let Some(ivec) = opt {
            Ok(Some(serde_json::from_slice(&ivec)?))
        } else {
            Ok(None)
        }
    }

    #[tracing::instrument]
    async fn cleanup<I: Identifier>(&self, identifier: &I) -> Result<(), Error> {
        let key = identifier.to_bytes()?;

        b!(self.identifier_details, identifier_details.remove(key));

        Ok(())
    }
}

type StreamItem = Result<IVec, Error>;
type LocalBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

fn hash_alias_key(hash: &IVec, alias: &Alias) -> Vec<u8> {
    let mut v = hash.to_vec();
    v.append(&mut alias.to_bytes());
    v
}

#[async_trait::async_trait(?Send)]
impl HashRepo for SledRepo {
    type Stream = LocalBoxStream<'static, StreamItem>;

    async fn hashes(&self) -> Self::Stream {
        let iter = self
            .hashes
            .iter()
            .keys()
            .map(|res| res.map_err(Error::from));

        Box::pin(from_iterator(iter))
    }

    #[tracing::instrument]
    async fn create(&self, hash: Self::Bytes) -> Result<Result<(), AlreadyExists>, Error> {
        let res = b!(self.hashes, {
            let hash2 = hash.clone();
            hashes.compare_and_swap(hash, None as Option<Self::Bytes>, Some(hash2))
        });

        Ok(res.map_err(|_| AlreadyExists))
    }

    #[tracing::instrument]
    async fn relate_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Error> {
        let key = hash_alias_key(&hash, alias);
        let value = alias.to_bytes();

        b!(self.hash_aliases, hash_aliases.insert(key, value));

        Ok(())
    }

    #[tracing::instrument]
    async fn remove_alias(&self, hash: Self::Bytes, alias: &Alias) -> Result<(), Error> {
        let key = hash_alias_key(&hash, alias);

        b!(self.hash_aliases, hash_aliases.remove(key));

        Ok(())
    }

    #[tracing::instrument]
    async fn aliases(&self, hash: Self::Bytes) -> Result<Vec<Alias>, Error> {
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

    #[tracing::instrument]
    async fn relate_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Error> {
        let bytes = identifier.to_bytes()?;

        b!(self.hash_identifiers, hash_identifiers.insert(hash, bytes));

        Ok(())
    }

    #[tracing::instrument]
    async fn identifier<I: Identifier + 'static>(&self, hash: Self::Bytes) -> Result<I, Error> {
        let opt = b!(self.hash_identifiers, hash_identifiers.get(hash));

        opt.ok_or(SledError::Missing)
            .map_err(Error::from)
            .and_then(|ivec| I::from_bytes(ivec.to_vec()))
    }

    #[tracing::instrument]
    async fn relate_variant_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        variant: String,
        identifier: &I,
    ) -> Result<(), Error> {
        let key = variant_key(&hash, &variant);
        let value = identifier.to_bytes()?;

        b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.insert(key, value)
        );

        Ok(())
    }

    #[tracing::instrument]
    async fn variant_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
        variant: String,
    ) -> Result<Option<I>, Error> {
        let key = variant_key(&hash, &variant);

        let opt = b!(
            self.hash_variant_identifiers,
            hash_variant_identifiers.get(key)
        );

        if let Some(ivec) = opt {
            Ok(Some(I::from_bytes(ivec.to_vec())?))
        } else {
            Ok(None)
        }
    }

    #[tracing::instrument]
    async fn variants<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Vec<(String, I)>, Error> {
        let vec = b!(
            self.hash_variant_identifiers,
            Ok(hash_variant_identifiers
                .scan_prefix(&hash)
                .filter_map(|res| res.ok())
                .filter_map(|(key, ivec)| {
                    let identifier = I::from_bytes(ivec.to_vec()).ok()?;
                    let variant = variant_from_key(&hash, &key)?;

                    Some((variant, identifier))
                })
                .collect::<Vec<_>>()) as Result<Vec<_>, sled::Error>
        );

        Ok(vec)
    }

    #[tracing::instrument]
    async fn relate_motion_identifier<I: Identifier>(
        &self,
        hash: Self::Bytes,
        identifier: &I,
    ) -> Result<(), Error> {
        let bytes = identifier.to_bytes()?;

        b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.insert(hash, bytes)
        );

        Ok(())
    }

    #[tracing::instrument]
    async fn motion_identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, Error> {
        let opt = b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.get(hash)
        );

        if let Some(ivec) = opt {
            Ok(Some(I::from_bytes(ivec.to_vec())?))
        } else {
            Ok(None)
        }
    }

    #[tracing::instrument]
    async fn cleanup(&self, hash: Self::Bytes) -> Result<(), Error> {
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
            Ok(()) as Result<(), sled::Error>
        });

        let variant_keys = b!(self.hash_variant_identifiers, {
            let v = hash_variant_identifiers
                .scan_prefix(hash)
                .keys()
                .filter_map(Result::ok)
                .collect::<Vec<_>>();

            Ok(v) as Result<Vec<_>, sled::Error>
        });
        b!(self.hash_variant_identifiers, {
            for key in variant_keys {
                let _ = hash_variant_identifiers.remove(key);
            }
            Ok(()) as Result<(), sled::Error>
        });

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl AliasRepo for SledRepo {
    #[tracing::instrument]
    async fn create(&self, alias: &Alias) -> Result<Result<(), AlreadyExists>, Error> {
        let bytes = alias.to_bytes();
        let bytes2 = bytes.clone();

        let res = b!(
            self.aliases,
            aliases.compare_and_swap(bytes, None as Option<Self::Bytes>, Some(bytes2))
        );

        Ok(res.map_err(|_| AlreadyExists))
    }

    #[tracing::instrument]
    async fn relate_delete_token(
        &self,
        alias: &Alias,
        delete_token: &DeleteToken,
    ) -> Result<Result<(), AlreadyExists>, Error> {
        let key = alias.to_bytes();
        let token = delete_token.to_bytes();

        let res = b!(
            self.alias_delete_tokens,
            alias_delete_tokens.compare_and_swap(key, None as Option<Self::Bytes>, Some(token))
        );

        Ok(res.map_err(|_| AlreadyExists))
    }

    #[tracing::instrument]
    async fn delete_token(&self, alias: &Alias) -> Result<DeleteToken, Error> {
        let key = alias.to_bytes();

        let opt = b!(self.alias_delete_tokens, alias_delete_tokens.get(key));

        opt.and_then(|ivec| DeleteToken::from_slice(&ivec))
            .ok_or(SledError::Missing)
            .map_err(Error::from)
    }

    #[tracing::instrument]
    async fn relate_hash(&self, alias: &Alias, hash: Self::Bytes) -> Result<(), Error> {
        let key = alias.to_bytes();

        b!(self.alias_hashes, alias_hashes.insert(key, hash));

        Ok(())
    }

    #[tracing::instrument]
    async fn hash(&self, alias: &Alias) -> Result<Self::Bytes, Error> {
        let key = alias.to_bytes();

        let opt = b!(self.alias_hashes, alias_hashes.get(key));

        opt.ok_or(SledError::Missing).map_err(Error::from)
    }

    #[tracing::instrument]
    async fn cleanup(&self, alias: &Alias) -> Result<(), Error> {
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
