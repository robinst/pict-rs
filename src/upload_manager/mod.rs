use crate::{
    config::Format,
    error::{Error, UploadError},
    ffmpeg::{InputFormat, ThumbnailFormat},
    magick::{details_hint, ValidInputType},
    migrate::{alias_id_key, alias_key, alias_key_bounds},
    store::{Identifier, Store},
};
use actix_web::web;
use sha2::Digest;
use std::{
    ops::{Deref, DerefMut},
    string::FromUtf8Error,
    sync::Arc,
};
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
//   - DEPRECATED:
//     - hash 2 variant path -> variant path
//     - hash 2 vairant path details -> details
// - Filename Tree
//   - filename -> hash
// - Details Tree
//   - filename / S::Identifier -> details
// - Path Tree
//   - filename -> S::Identifier
//   - filename / variant path -> S::Identifier
//   - filename / motion -> S::Identifier

#[derive(Clone)]
pub(crate) struct UploadManager<S> {
    inner: Arc<UploadManagerInner>,
    store: S,
}

pub(crate) struct UploadManagerInner {
    format: Option<Format>,
    hasher: sha2::Sha256,
    pub(crate) alias_tree: sled::Tree,
    pub(crate) filename_tree: sled::Tree,
    pub(crate) main_tree: sled::Tree,
    details_tree: sled::Tree,
    pub(crate) identifier_tree: sled::Tree,
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

impl<S> UploadManager<S>
where
    S: Store + 'static,
    Error: From<S::Error>,
{
    /// Create a new UploadManager
    pub(crate) async fn new(store: S, db: sled::Db, format: Option<Format>) -> Result<Self, Error> {
        let manager = UploadManager {
            inner: Arc::new(UploadManagerInner {
                format,
                hasher: sha2::Sha256::new(),
                alias_tree: db.open_tree("alias")?,
                filename_tree: db.open_tree("filename")?,
                details_tree: db.open_tree("details")?,
                main_tree: db.open_tree("main")?,
                identifier_tree: db.open_tree("path")?,
                db,
            }),
            store,
        };

        Ok(manager)
    }

    pub(crate) fn store(&self) -> &S {
        &self.store
    }

    pub(crate) fn inner(&self) -> &UploadManagerInner {
        &self.inner
    }

    pub(crate) async fn still_identifier_from_filename(
        &self,
        filename: String,
    ) -> Result<S::Identifier, Error> {
        let identifier = self.identifier_from_filename(filename.clone()).await?;
        let details = if let Some(details) = self
            .variant_details(identifier.clone(), filename.clone())
            .await?
        {
            details
        } else {
            let hint = details_hint(&filename);
            Details::from_store(self.store.clone(), identifier.clone(), hint).await?
        };

        if !details.is_motion() {
            return Ok(identifier);
        }

        if let Some(motion_identifier) = self.motion_identifier(&filename).await? {
            return Ok(motion_identifier);
        }

        let permit = crate::PROCESS_SEMAPHORE.acquire().await;
        let mut reader = crate::ffmpeg::thumbnail(
            self.store.clone(),
            identifier,
            InputFormat::Mp4,
            ThumbnailFormat::Jpeg,
        )
        .await?;
        let motion_identifier = self.store.save_async_read(&mut reader).await?;
        drop(permit);

        self.store_motion_path(&filename, &motion_identifier)
            .await?;
        Ok(motion_identifier)
    }

    async fn motion_identifier(&self, filename: &str) -> Result<Option<S::Identifier>, Error> {
        let identifier_tree = self.inner.identifier_tree.clone();
        let motion_key = format!("{}/motion", filename);

        let opt = web::block(move || identifier_tree.get(motion_key.as_bytes())).await??;

        if let Some(ivec) = opt {
            return Ok(Some(S::Identifier::from_bytes(ivec.to_vec())?));
        }

        Ok(None)
    }

    async fn store_motion_path(
        &self,
        filename: &str,
        identifier: &S::Identifier,
    ) -> Result<(), Error> {
        let identifier_bytes = identifier.to_bytes()?;
        let motion_key = format!("{}/motion", filename);
        let identifier_tree = self.inner.identifier_tree.clone();

        web::block(move || identifier_tree.insert(motion_key.as_bytes(), identifier_bytes))
            .await??;
        Ok(())
    }

    #[instrument(skip(self))]
    pub(crate) async fn identifier_from_filename(
        &self,
        filename: String,
    ) -> Result<S::Identifier, Error> {
        let identifier_tree = self.inner.identifier_tree.clone();
        let path_ivec = web::block(move || identifier_tree.get(filename.as_bytes()))
            .await??
            .ok_or(UploadError::MissingFile)?;

        let identifier = S::Identifier::from_bytes(path_ivec.to_vec())?;

        Ok(identifier)
    }

    #[instrument(skip(self))]
    async fn store_identifier(
        &self,
        filename: String,
        identifier: &S::Identifier,
    ) -> Result<(), Error> {
        let identifier_bytes = identifier.to_bytes()?;
        let identifier_tree = self.inner.identifier_tree.clone();
        web::block(move || identifier_tree.insert(filename.as_bytes(), identifier_bytes)).await??;
        Ok(())
    }

    #[instrument(skip(self))]
    pub(crate) async fn variant_identifier(
        &self,
        process_path: &std::path::Path,
        filename: &str,
    ) -> Result<Option<S::Identifier>, Error> {
        let key = self.variant_key(process_path, filename)?;
        let identifier_tree = self.inner.identifier_tree.clone();
        let path_opt = web::block(move || identifier_tree.get(key)).await??;

        if let Some(ivec) = path_opt {
            let identifier = S::Identifier::from_bytes(ivec.to_vec())?;
            Ok(Some(identifier))
        } else {
            Ok(None)
        }
    }

    /// Store the path to a generated image variant so we can easily clean it up later
    #[instrument(skip(self))]
    pub(crate) async fn store_variant(
        &self,
        variant_process_path: Option<&std::path::Path>,
        identifier: &S::Identifier,
        filename: &str,
    ) -> Result<(), Error> {
        let key = if let Some(path) = variant_process_path {
            self.variant_key(path, filename)?
        } else {
            let mut vec = filename.as_bytes().to_vec();
            vec.extend(b"/");
            vec.extend(&identifier.to_bytes()?);
            vec
        };
        let identifier_tree = self.inner.identifier_tree.clone();
        let identifier_bytes = identifier.to_bytes()?;

        debug!("Storing variant");
        web::block(move || identifier_tree.insert(key, identifier_bytes)).await??;
        debug!("Stored variant");

        Ok(())
    }

    /// Get the image details for a given variant
    #[instrument(skip(self))]
    pub(crate) async fn variant_details(
        &self,
        identifier: S::Identifier,
        filename: String,
    ) -> Result<Option<Details>, Error> {
        let key = self.details_key(identifier, &filename)?;
        let details_tree = self.inner.details_tree.clone();

        debug!("Getting details");
        let opt = match web::block(move || details_tree.get(key)).await?? {
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
        identifier: S::Identifier,
        filename: String,
        details: &Details,
    ) -> Result<(), Error> {
        let key = self.details_key(identifier, &filename)?;
        let details_tree = self.inner.details_tree.clone();
        let details_value = serde_json::to_vec(details)?;

        debug!("Storing details");
        web::block(move || details_tree.insert(key, details_value)).await??;
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
                    .ok_or_else(|| trans_upload_error(UploadError::MissingAlias))?;

                // Bail if invalid token
                if existing_token != token {
                    warn!("Invalid delete token");
                    return Err(trans_upload_error(UploadError::InvalidToken));
                }

                // -- GET ID FOR HASH TREE CLEANUP --
                debug!("Deleting alias -> id mapping");
                let id = alias_tree
                    .remove(alias_id_key(&alias2).as_bytes())?
                    .ok_or_else(|| trans_upload_error(UploadError::MissingAlias))?;
                let id = String::from_utf8(id.to_vec()).map_err(trans_utf8_error)?;

                // -- GET HASH FOR HASH TREE CLEANUP --
                debug!("Deleting alias -> hash mapping");
                let hash = alias_tree
                    .remove(alias2.as_bytes())?
                    .ok_or_else(|| trans_upload_error(UploadError::MissingAlias))?;

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

    pub(crate) fn session(&self) -> UploadManagerSession<S> {
        UploadManagerSession::new(self.clone())
    }

    // Find image variants and remove them from the DB and the disk
    #[instrument(skip(self))]
    async fn cleanup_files(&self, filename: FilenameIVec) -> Result<(), Error> {
        let filename = filename.inner;

        let filename2 = filename.clone();
        let identifier_tree = self.inner.identifier_tree.clone();
        let identifier = web::block(move || identifier_tree.remove(filename2)).await??;

        let mut errors = Vec::new();
        if let Some(identifier) = identifier {
            let identifier = S::Identifier::from_bytes(identifier.to_vec())?;
            debug!("Deleting {:?}", identifier);
            if let Err(e) = self.store.remove(&identifier).await {
                errors.push(e.into());
            }
        }

        let filename2 = filename.clone();
        let fname_tree = self.inner.filename_tree.clone();
        debug!("Deleting filename -> hash mapping");
        web::block(move || fname_tree.remove(filename2)).await??;

        let path_prefix = filename.clone();
        let identifier_tree = self.inner.identifier_tree.clone();
        debug!("Fetching file variants");
        let identifiers = web::block(move || {
            identifier_tree
                .scan_prefix(path_prefix)
                .values()
                .collect::<Result<Vec<sled::IVec>, sled::Error>>()
        })
        .await??;

        debug!("{} files prepared for deletion", identifiers.len());

        for id in identifiers {
            let identifier = S::Identifier::from_bytes(id.to_vec())?;

            debug!("Deleting {:?}", identifier);
            if let Err(e) = self.store.remove(&identifier).await {
                errors.push(e);
            }
        }

        let path_prefix = filename.clone();
        let identifier_tree = self.inner.identifier_tree.clone();
        debug!("Deleting path info");
        web::block(move || {
            for res in identifier_tree.scan_prefix(path_prefix).keys() {
                let key = res?;
                identifier_tree.remove(key)?;
            }
            Ok(()) as Result<(), Error>
        })
        .await??;

        for error in errors {
            error!("Error deleting files, {}", error);
        }
        Ok(())
    }

    pub(crate) fn variant_key(
        &self,
        variant_process_path: &std::path::Path,
        filename: &str,
    ) -> Result<Vec<u8>, Error> {
        let path_string = variant_process_path
            .to_str()
            .ok_or(UploadError::Path)?
            .to_string();

        let vec = format!("{}/{}", filename, path_string).as_bytes().to_vec();
        Ok(vec)
    }

    fn details_key(&self, identifier: S::Identifier, filename: &str) -> Result<Vec<u8>, Error> {
        let mut vec = filename.as_bytes().to_vec();
        vec.extend(b"/");
        vec.extend(&identifier.to_bytes()?);

        Ok(vec)
    }
}

impl<T> Serde<T> {
    pub(crate) fn new(inner: T) -> Self {
        Serde { inner }
    }
}

impl Details {
    fn is_motion(&self) -> bool {
        self.content_type.type_() == "video"
            || self.content_type.type_() == "image" && self.content_type.subtype() == "gif"
    }

    #[tracing::instrument("Details from bytes", skip(input))]
    pub(crate) async fn from_bytes(
        input: web::Bytes,
        hint: Option<ValidInputType>,
    ) -> Result<Self, Error> {
        let details = crate::magick::details_bytes(input, hint).await?;

        Ok(Details::now(
            details.width,
            details.height,
            details.mime_type,
        ))
    }

    #[tracing::instrument("Details from store")]
    pub(crate) async fn from_store<S: Store>(
        store: S,
        identifier: S::Identifier,
        expected_format: Option<ValidInputType>,
    ) -> Result<Self, Error> {
        let details = crate::magick::details_store(store, identifier, expected_format).await?;

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

fn trans_upload_error(
    upload_error: UploadError,
) -> sled::transaction::ConflictableTransactionError<Error> {
    trans_err(upload_error)
}

fn trans_utf8_error(e: FromUtf8Error) -> sled::transaction::ConflictableTransactionError<Error> {
    trans_err(e)
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

impl<T> Deref for Serde<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for Serde<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<S> std::fmt::Debug for UploadManager<S> {
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
