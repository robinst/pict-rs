use crate::{
    repo_04::{
        Alias, AliasRepo, BaseRepo, DeleteToken, Details, HashRepo, Identifier, IdentifierRepo,
        RepoError, SettingsRepo,
    },
    store::StoreError,
    stream::{from_iterator, LocalBoxStream},
};
use sled::{Db, IVec, Tree};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

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
    alias_delete_tokens: Tree,
    _db: Db,
}

impl SledRepo {
    #[tracing::instrument]
    pub(crate) fn build(path: PathBuf, cache_capacity: u64) -> color_eyre::Result<Option<Self>> {
        let Some(db) = Self::open(path, cache_capacity)? else {
            return Ok(None);
        };

        Ok(Some(SledRepo {
            healthz_count: Arc::new(AtomicU64::new(0)),
            healthz: db.open_tree("pict-rs-healthz-tree")?,
            settings: db.open_tree("pict-rs-settings-tree")?,
            identifier_details: db.open_tree("pict-rs-identifier-details-tree")?,
            hashes: db.open_tree("pict-rs-hashes-tree")?,
            hash_aliases: db.open_tree("pict-rs-hash-aliases-tree")?,
            hash_identifiers: db.open_tree("pict-rs-hash-identifiers-tree")?,
            hash_variant_identifiers: db.open_tree("pict-rs-hash-variant-identifiers-tree")?,
            hash_motion_identifiers: db.open_tree("pict-rs-hash-motion-identifiers-tree")?,
            alias_delete_tokens: db.open_tree("pict-rs-alias-delete-tokens-tree")?,
            _db: db,
        }))
    }

    fn open(mut path: PathBuf, cache_capacity: u64) -> Result<Option<Db>, SledError> {
        path.push("v0.4.0-alpha.1");

        if let Err(e) = std::fs::metadata(&path) {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
        }

        let db = ::sled::Config::new()
            .cache_capacity(cache_capacity)
            .path(path)
            .open()?;

        Ok(Some(db))
    }

    pub(crate) async fn health_check(&self) -> Result<(), RepoError> {
        let next = self.healthz_count.fetch_add(1, Ordering::Relaxed);
        b!(self.healthz, {
            healthz.insert("healthz", &next.to_be_bytes()[..])
        });
        self.healthz.flush_async().await.map_err(SledError::from)?;
        b!(self.healthz, healthz.get("healthz"));
        Ok(())
    }
}

impl BaseRepo for SledRepo {
    type Bytes = IVec;
}

#[async_trait::async_trait(?Send)]
impl SettingsRepo for SledRepo {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn get(&self, key: &'static str) -> Result<Option<Self::Bytes>, RepoError> {
        let opt = b!(self.settings, settings.get(key));

        Ok(opt)
    }
}

fn variant_from_key(hash: &[u8], key: &[u8]) -> Option<String> {
    let prefix_len = hash.len() + 1;
    let variant_bytes = key.get(prefix_len..)?.to_vec();
    String::from_utf8(variant_bytes).ok()
}

#[async_trait::async_trait(?Send)]
impl IdentifierRepo for SledRepo {
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
}

type StreamItem = Result<IVec, RepoError>;

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
    async fn identifier<I: Identifier + 'static>(
        &self,
        hash: Self::Bytes,
    ) -> Result<Option<I>, StoreError> {
        let Some(ivec) = b!(self.hash_identifiers, hash_identifiers.get(hash)) else {
            return Ok(None);
        };

        Ok(Some(I::from_bytes(ivec.to_vec())?))
    }

    #[tracing::instrument(level = "debug", skip(self, hash), fields(hash = hex::encode(&hash)))]
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
}

#[async_trait::async_trait(?Send)]
impl AliasRepo for SledRepo {
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

    #[tracing::instrument(skip_all)]
    async fn for_hash(&self, hash: Self::Bytes) -> Result<Vec<Alias>, RepoError> {
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
