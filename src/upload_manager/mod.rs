use crate::{
    config::Format,
    error::{Error, UploadError},
    migrate::{alias_id_key, alias_key, alias_key_bounds, variant_key_bounds, LatestDb},
};
use actix_web::web;
use sha2::Digest;
use std::{path::PathBuf, sync::Arc};
use tracing::{debug, error, info, instrument, warn, Span};
use tracing_futures::Instrument;

mod hasher;
mod session;

pub(super) use session::UploadManagerSession;

// TREE STRUCTURE
// - Alias Tree
//   - alias -> hash
//   - alias / id -> u64(id)
//   - alias / delete -> delete token
// - Main Tree
//   - hash -> filename
//   - hash 0 u64(id) -> alias
//   - hash 2 variant path -> variant path
// - Filename Tree
//   - filename -> hash
// - Path Tree
//   - filename -> relative path
//   - filename / variant operation path -> relative path
// - Settings Tree
//   - last-path -> last generated path
//   - fs-restructure-01-started -> bool
//   - fs-restructure-01-complete -> bool

#[derive(Clone)]
pub struct UploadManager {
    inner: Arc<UploadManagerInner>,
}

struct UploadManagerInner {
    format: Option<Format>,
    hasher: sha2::Sha256,
    image_dir: PathBuf,
    alias_tree: sled::Tree,
    filename_tree: sled::Tree,
    main_tree: sled::Tree,
    path_tree: sled::Tree,
    settings_tree: sled::Tree,
    db: sled::Db,
}

#[derive(Clone, Debug)]
pub(crate) struct Serde<T> {
    inner: T,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Details {
    width: usize,
    height: usize,
    content_type: Serde<mime::Mime>,
    created_at: time::OffsetDateTime,
}

struct FilenameIVec {
    inner: sled::IVec,
}

impl UploadManager {
    /// Get the image directory
    pub(crate) fn image_dir(&self) -> PathBuf {
        self.inner.image_dir.clone()
    }

    /// Create a new UploadManager
    pub(crate) async fn new(mut root_dir: PathBuf, format: Option<Format>) -> Result<Self, Error> {
        let root_clone = root_dir.clone();
        // sled automatically creates it's own directories
        let db = web::block(move || LatestDb::exists(root_clone).migrate()).await??;

        root_dir.push("files");

        // Ensure file dir exists
        tokio::fs::create_dir_all(&root_dir).await?;

        let settings_tree = db.open_tree("settings")?;

        let manager = UploadManager {
            inner: Arc::new(UploadManagerInner {
                format,
                hasher: sha2::Sha256::new(),
                image_dir: root_dir,
                alias_tree: db.open_tree("alias")?,
                filename_tree: db.open_tree("filename")?,
                main_tree: db.open_tree("main")?,
                path_tree: db.open_tree("path")?,
                settings_tree,
                db,
            }),
        };

        Ok(manager)
    }

    /// Store the path to a generated image variant so we can easily clean it up later
    #[instrument(skip(self))]
    pub(crate) async fn store_variant(&self, path: PathBuf, filename: String) -> Result<(), Error> {
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Getting hash");
        let hash: sled::IVec = web::block(move || fname_tree.get(filename.as_bytes()))
            .await??
            .ok_or(UploadError::MissingFilename)?;

        let key = variant_key(&hash, &path_string);
        let main_tree = self.inner.main_tree.clone();
        debug!("Storing variant");
        web::block(move || main_tree.insert(key, path_string.as_bytes())).await??;
        debug!("Stored variant");

        Ok(())
    }

    /// Get the image details for a given variant
    #[instrument(skip(self))]
    pub(crate) async fn variant_details(
        &self,
        path: PathBuf,
        filename: String,
    ) -> Result<Option<Details>, Error> {
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Getting hash");
        let hash: sled::IVec = web::block(move || fname_tree.get(filename.as_bytes()))
            .await??
            .ok_or(UploadError::MissingFilename)?;

        let key = variant_details_key(&hash, &path_string);
        let main_tree = self.inner.main_tree.clone();
        debug!("Getting details");
        let opt = match web::block(move || main_tree.get(key)).await?? {
            Some(ivec) => match serde_json::from_slice(&ivec) {
                Ok(details) => Some(details),
                Err(_) => None,
            },
            None => None,
        };
        debug!("Got details");

        Ok(opt)
    }

    #[instrument(skip(self))]
    pub(crate) async fn store_variant_details(
        &self,
        path: PathBuf,
        filename: String,
        details: &Details,
    ) -> Result<(), Error> {
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Getting hash");
        let hash: sled::IVec = web::block(move || fname_tree.get(filename.as_bytes()))
            .await??
            .ok_or(UploadError::MissingFilename)?;

        let key = variant_details_key(&hash, &path_string);
        let main_tree = self.inner.main_tree.clone();
        let details_value = serde_json::to_string(details)?;
        debug!("Storing details");
        web::block(move || main_tree.insert(key, details_value.as_bytes())).await??;
        debug!("Stored details");

        Ok(())
    }

    /// Get a list of aliases for a given file
    pub(crate) async fn aliases_by_filename(&self, filename: String) -> Result<Vec<String>, Error> {
        let fname_tree = self.inner.filename_tree.clone();
        let hash = web::block(move || fname_tree.get(filename.as_bytes()))
            .await??
            .ok_or(UploadError::MissingAlias)?;

        self.aliases_by_hash(&hash).await
    }

    /// Get a list of aliases for a given alias
    pub(crate) async fn aliases_by_alias(&self, alias: String) -> Result<Vec<String>, Error> {
        let alias_tree = self.inner.alias_tree.clone();
        let hash = web::block(move || alias_tree.get(alias.as_bytes()))
            .await??
            .ok_or(UploadError::MissingFilename)?;

        self.aliases_by_hash(&hash).await
    }

    async fn aliases_by_hash(&self, hash: &sled::IVec) -> Result<Vec<String>, Error> {
        let (start, end) = alias_key_bounds(hash);
        let main_tree = self.inner.main_tree.clone();
        let aliases = web::block(move || {
            main_tree
                .range(start..end)
                .values()
                .collect::<Result<Vec<_>, _>>()
        })
        .await??;

        debug!("Got {} aliases for hash", aliases.len());
        let aliases = aliases
            .into_iter()
            .filter_map(|s| String::from_utf8(s.to_vec()).ok())
            .collect::<Vec<_>>();

        for alias in aliases.iter() {
            debug!("{}", alias);
        }

        Ok(aliases)
    }

    /// Delete an alias without a delete token
    pub(crate) async fn delete_without_token(&self, alias: String) -> Result<(), Error> {
        let token_key = delete_key(&alias);
        let alias_tree = self.inner.alias_tree.clone();
        let token = web::block(move || alias_tree.get(token_key.as_bytes()))
            .await??
            .ok_or(UploadError::MissingAlias)?;

        self.delete(alias, String::from_utf8(token.to_vec())?).await
    }

    /// Delete the alias, and the file & variants if no more aliases exist
    #[instrument(skip(self, alias, token))]
    pub(crate) async fn delete(&self, alias: String, token: String) -> Result<(), Error> {
        use sled::Transactional;
        let main_tree = self.inner.main_tree.clone();
        let alias_tree = self.inner.alias_tree.clone();

        let span = Span::current();
        let alias2 = alias.clone();
        let hash = web::block(move || {
            [&main_tree, &alias_tree].transaction(|v| {
                let entered = span.enter();
                let main_tree = &v[0];
                let alias_tree = &v[1];

                // -- GET TOKEN --
                debug!("Deleting alias -> delete-token mapping");
                let existing_token = alias_tree
                    .remove(delete_key(&alias2).as_bytes())?
                    .ok_or_else(|| trans_err(UploadError::MissingAlias))?;

                // Bail if invalid token
                if existing_token != token {
                    warn!("Invalid delete token");
                    return Err(trans_err(UploadError::InvalidToken));
                }

                // -- GET ID FOR HASH TREE CLEANUP --
                debug!("Deleting alias -> id mapping");
                let id = alias_tree
                    .remove(alias_id_key(&alias2).as_bytes())?
                    .ok_or_else(|| trans_err(UploadError::MissingAlias))?;
                let id = String::from_utf8(id.to_vec()).map_err(trans_err)?;

                // -- GET HASH FOR HASH TREE CLEANUP --
                debug!("Deleting alias -> hash mapping");
                let hash = alias_tree
                    .remove(alias2.as_bytes())?
                    .ok_or_else(|| trans_err(UploadError::MissingAlias))?;

                // -- REMOVE HASH TREE ELEMENT --
                debug!("Deleting hash -> alias mapping");
                main_tree.remove(alias_key(&hash, &id))?;
                drop(entered);
                Ok(hash)
            })
        })
        .await??;

        self.check_delete_files(hash).await
    }

    async fn check_delete_files(&self, hash: sled::IVec) -> Result<(), Error> {
        // -- CHECK IF ANY OTHER ALIASES EXIST --
        let main_tree = self.inner.main_tree.clone();
        let (start, end) = alias_key_bounds(&hash);
        debug!("Checking for additional aliases referencing hash");
        let any_aliases = web::block(move || {
            Ok(main_tree.range(start..end).next().is_some()) as Result<bool, Error>
        })
        .await??;

        // Bail if there are existing aliases
        if any_aliases {
            debug!("Other aliases reference file, not removing from disk");
            return Ok(());
        }

        // -- DELETE HASH ENTRY --
        let main_tree = self.inner.main_tree.clone();
        let hash2 = hash.clone();
        debug!("Deleting hash -> filename mapping");
        let filename = web::block(move || main_tree.remove(&hash2))
            .await??
            .ok_or(UploadError::MissingFile)?;

        // -- DELETE FILES --
        let this = self.clone();
        let cleanup_span = tracing::info_span!(
            parent: None,
            "Cleanup",
            filename = &tracing::field::display(String::from_utf8_lossy(&filename)),
        );
        cleanup_span.follows_from(Span::current());
        debug!("Spawning cleanup task");
        actix_rt::spawn(
            async move {
                if let Err(e) = this
                    .cleanup_files(FilenameIVec::new(filename.clone()))
                    .await
                {
                    error!("Error removing files from fs, {}", e);
                }
                info!(
                    "Files deleted for {:?}",
                    String::from_utf8(filename.to_vec())
                );
            }
            .instrument(cleanup_span),
        );

        Ok(())
    }

    /// Fetch the real on-disk filename given an alias
    #[instrument(skip(self))]
    pub(crate) async fn from_alias(&self, alias: String) -> Result<String, Error> {
        let tree = self.inner.alias_tree.clone();
        debug!("Getting hash from alias");
        let hash = web::block(move || tree.get(alias.as_bytes()))
            .await??
            .ok_or(UploadError::MissingAlias)?;

        let main_tree = self.inner.main_tree.clone();
        debug!("Getting filename from hash");
        let filename = web::block(move || main_tree.get(hash))
            .await??
            .ok_or(UploadError::MissingFile)?;

        let filename = String::from_utf8(filename.to_vec())?;

        Ok(filename)
    }

    pub(crate) fn session(&self) -> UploadManagerSession {
        UploadManagerSession::new(self.clone())
    }

    // Find image variants and remove them from the DB and the disk
    #[instrument(skip(self))]
    async fn cleanup_files(&self, filename: FilenameIVec) -> Result<(), Error> {
        let filename = filename.inner;
        let mut path = self.image_dir();
        let fname = String::from_utf8(filename.to_vec())?;
        path.push(fname);

        let mut errors = Vec::new();
        debug!("Deleting {:?}", path);
        if let Err(e) = tokio::fs::remove_file(path).await {
            errors.push(e.into());
        }

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Deleting filename -> hash mapping");
        let hash = web::block(move || fname_tree.remove(filename))
            .await??
            .ok_or(UploadError::MissingFile)?;

        let (start, end) = variant_key_bounds(&hash);
        let main_tree = self.inner.main_tree.clone();
        debug!("Fetching file variants");
        let keys = web::block(move || {
            let mut keys = Vec::new();
            for key in main_tree.range(start..end).keys() {
                keys.push(key?.to_owned());
            }

            Ok(keys) as Result<Vec<sled::IVec>, Error>
        })
        .await??;

        debug!("{} files prepared for deletion", keys.len());

        for key in keys {
            let main_tree = self.inner.main_tree.clone();
            if let Some(path) = web::block(move || main_tree.remove(key)).await?? {
                let s = String::from_utf8_lossy(&path);
                debug!("Deleting {}", s);
                // ignore json objects
                if !s.starts_with('{') {
                    if let Err(e) = remove_path(path).await {
                        errors.push(e);
                    }
                }
            }
        }

        for error in errors {
            error!("Error deleting files, {}", error);
        }
        Ok(())
    }
}

impl<T> Serde<T> {
    pub(crate) fn new(inner: T) -> Self {
        Serde { inner }
    }
}

impl Details {
    #[tracing::instrument("Details from bytes", skip(input))]
    pub(crate) async fn from_bytes(input: web::Bytes) -> Result<Self, Error> {
        let details = crate::magick::details_bytes(input).await?;

        Ok(Details::now(
            details.width,
            details.height,
            details.mime_type,
        ))
    }

    #[tracing::instrument("Details from path", fields(path = &tracing::field::debug(&path.as_ref())))]
    pub(crate) async fn from_path<P>(path: P) -> Result<Self, Error>
    where
        P: AsRef<std::path::Path>,
    {
        let details = crate::magick::details(&path).await?;

        Ok(Details::now(
            details.width,
            details.height,
            details.mime_type,
        ))
    }

    fn now(width: usize, height: usize, content_type: mime::Mime) -> Self {
        Details {
            width,
            height,
            content_type: Serde::new(content_type),
            created_at: time::OffsetDateTime::now_utc(),
        }
    }

    pub(crate) fn content_type(&self) -> mime::Mime {
        self.content_type.inner.clone()
    }

    pub(crate) fn system_time(&self) -> std::time::SystemTime {
        self.created_at.into()
    }
}

impl FilenameIVec {
    fn new(inner: sled::IVec) -> Self {
        FilenameIVec { inner }
    }
}

async fn remove_path(path: sled::IVec) -> Result<(), Error> {
    let path_string = String::from_utf8(path.to_vec())?;
    tokio::fs::remove_file(path_string).await?;
    Ok(())
}

fn trans_err<E>(e: E) -> sled::transaction::ConflictableTransactionError<Error>
where
    Error: From<E>,
{
    sled::transaction::ConflictableTransactionError::Abort(e.into())
}

fn delete_key(alias: &str) -> String {
    format!("{}/delete", alias)
}

fn variant_key(hash: &[u8], path: &str) -> Vec<u8> {
    let mut key = hash.to_vec();
    key.extend(&[2]);
    key.extend(path.as_bytes());
    key
}

fn variant_details_key(hash: &[u8], path: &str) -> Vec<u8> {
    let mut key = hash.to_vec();
    key.extend(&[2]);
    key.extend(path.as_bytes());
    key.extend(b"details");
    key
}

impl std::fmt::Debug for UploadManager {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("UploadManager").finish()
    }
}

impl<T> serde::Serialize for Serde<T>
where
    T: std::fmt::Display,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = self.inner.to_string();
        serde::Serialize::serialize(s.as_str(), serializer)
    }
}

impl<'de, T> serde::Deserialize<'de> for Serde<T>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = serde::Deserialize::deserialize(deserializer)?;
        let inner = s
            .parse::<T>()
            .map_err(|e| serde::de::Error::custom(e.to_string()))?;

        Ok(Serde { inner })
    }
}

impl std::fmt::Debug for FilenameIVec {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", String::from_utf8(self.inner.to_vec()))
    }
}
