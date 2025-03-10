use crate::{
    details::HumanDate,
    repo_04::{
        Alias, AliasRepo, BaseRepo, DeleteToken, Details, HashRepo, IdentifierRepo, RepoError,
        SettingsRepo,
    },
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

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
enum MaybeHumanDate {
    HumanDate(#[serde(with = "time::serde::rfc3339")] time::OffsetDateTime),
    OldDate(#[serde(serialize_with = "time::serde::rfc3339::serialize")] time::OffsetDateTime),
}

impl MaybeHumanDate {
    fn into_human_date(self) -> HumanDate {
        match self {
            Self::HumanDate(timestamp) | Self::OldDate(timestamp) => HumanDate { timestamp },
        }
    }
}

macro_rules! b {
    ($self:ident.$ident:ident, $expr:expr) => {{
        let $ident = $self.$ident.clone();

        crate::sync::spawn_blocking("04-sled-io", move || $expr)
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

    #[error("Error reading string")]
    Utf8(#[from] std::str::Utf8Error),
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

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct OldDetails {
    width: u16,
    height: u16,
    frames: Option<u32>,
    content_type: crate::serde_str::Serde<mime::Mime>,
    created_at: MaybeHumanDate,
}

impl OldDetails {
    fn into_details(self) -> Option<Details> {
        let OldDetails {
            width,
            height,
            frames,
            content_type,
            created_at,
        } = self;

        let format = crate::formats::InternalFormat::maybe_from_media_type(
            &content_type,
            self.frames.is_some(),
        )?;

        Some(Details::from_parts_full(
            format,
            width,
            height,
            frames,
            created_at.into_human_date(),
        ))
    }
}

impl SledRepo {
    #[tracing::instrument]
    pub(crate) fn build(path: PathBuf) -> color_eyre::Result<Option<Self>> {
        let Some(db) = Self::open(path)? else {
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

    fn open(mut path: PathBuf) -> Result<Option<Db>, SledError> {
        path.push("v0.4.0-alpha.1");

        if let Err(e) = std::fs::metadata(&path) {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
        }

        let db = ::sled::Config::new().path(path).open()?;

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
    #[tracing::instrument(level = "trace", skip(self))]
    async fn details(&self, key: Arc<str>) -> Result<Option<Details>, RepoError> {
        let opt = b!(
            self.identifier_details,
            identifier_details.get(key.as_bytes())
        );

        opt.map(|ivec| serde_json::from_slice::<OldDetails>(&ivec))
            .transpose()
            .map_err(SledError::from)
            .map_err(RepoError::from)
            .map(|opt| opt.and_then(OldDetails::into_details))
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
    async fn identifier(&self, hash: Self::Bytes) -> Result<Option<Arc<str>>, RepoError> {
        let Some(ivec) = b!(self.hash_identifiers, hash_identifiers.get(hash)) else {
            return Ok(None);
        };

        Ok(Some(Arc::from(
            std::str::from_utf8(&ivec[..])
                .map_err(SledError::from)?
                .to_string(),
        )))
    }

    #[tracing::instrument(level = "debug", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn variants(&self, hash: Self::Bytes) -> Result<Vec<(String, Arc<str>)>, RepoError> {
        let vec = b!(
            self.hash_variant_identifiers,
            Ok(hash_variant_identifiers
                .scan_prefix(&hash)
                .filter_map(|res| res.ok())
                .filter_map(|(key, ivec)| {
                    let identifier = String::from_utf8(ivec.to_vec()).ok();
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

                    Some((variant?, Arc::from(identifier?)))
                })
                .collect::<Vec<_>>()) as Result<Vec<_>, SledError>
        );

        Ok(vec)
    }

    #[tracing::instrument(level = "trace", skip(self, hash), fields(hash = hex::encode(&hash)))]
    async fn motion_identifier(&self, hash: Self::Bytes) -> Result<Option<Arc<str>>, RepoError> {
        let opt = b!(
            self.hash_motion_identifiers,
            hash_motion_identifiers.get(hash)
        );

        opt.map(|ivec| {
            Ok(Arc::from(
                std::str::from_utf8(&ivec[..])
                    .map_err(SledError::from)?
                    .to_string(),
            ))
        })
        .transpose()
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
    async fn aliases_for_hash(&self, hash: Self::Bytes) -> Result<Vec<Alias>, RepoError> {
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

impl From<tokio::task::JoinError> for SledError {
    fn from(_: tokio::task::JoinError) -> Self {
        SledError::Panic
    }
}
