use crate::{
    config::Format,
    error::{Error, UploadError},
    migrate::{alias_id_key, alias_key, alias_key_bounds, variant_key_bounds, LatestDb},
    to_ext,
};
use actix_web::web;
use futures_util::stream::{LocalBoxStream, StreamExt};
use sha2::Digest;
use std::{
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};
use tracing::{debug, error, info, instrument, warn, Span};
use tracing_futures::Instrument;

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

#[derive(Clone)]
pub struct UploadManager {
    inner: Arc<UploadManagerInner>,
}

pub struct UploadManagerSession {
    manager: UploadManager,
    alias: Option<String>,
    finished: bool,
}

impl UploadManagerSession {
    pub(crate) fn succeed(mut self) {
        self.finished = true;
    }

    pub(crate) fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }
}

impl Drop for UploadManagerSession {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        if let Some(alias) = self.alias.take() {
            let manager = self.manager.clone();
            let cleanup_span = tracing::info_span!(
                parent: None,
                "Upload cleanup",
                alias = &tracing::field::display(&alias),
            );
            cleanup_span.follows_from(Span::current());
            actix_rt::spawn(
                async move {
                    // undo alias -> hash mapping
                    debug!("Remove alias -> hash mapping");
                    if let Ok(Some(hash)) = manager.inner.alias_tree.remove(&alias) {
                        // undo alias -> id mapping
                        debug!("Remove alias -> id mapping");
                        let key = alias_id_key(&alias);
                        if let Ok(Some(id)) = manager.inner.alias_tree.remove(&key) {
                            // undo hash/id -> alias mapping
                            debug!("Remove hash/id -> alias mapping");
                            let id = String::from_utf8_lossy(&id);
                            let key = alias_key(&hash, &id);
                            let _ = manager.inner.main_tree.remove(&key);
                        }

                        let _ = manager.check_delete_files(hash).await;
                    }
                }
                .instrument(cleanup_span),
            );
        }
    }
}

pub struct Hasher<I, D> {
    inner: I,
    hasher: D,
}

impl<I, D> Hasher<I, D>
where
    D: Digest + Send + 'static,
{
    fn new(reader: I, digest: D) -> Self {
        Hasher {
            inner: reader,
            hasher: digest,
        }
    }

    async fn finalize_reset(self) -> Result<Hash, Error> {
        let mut hasher = self.hasher;
        let hash = web::block(move || Hash::new(hasher.finalize_reset().to_vec())).await?;
        Ok(hash)
    }
}

impl<I, D> AsyncRead for Hasher<I, D>
where
    I: AsyncRead + Unpin,
    D: Digest + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before_len = buf.filled().len();
        let poll_res = Pin::new(&mut self.inner).poll_read(cx, buf);
        let after_len = buf.filled().len();
        if after_len > before_len {
            self.hasher.update(&buf.filled()[before_len..after_len]);
        }
        poll_res
    }
}

struct UploadManagerInner {
    format: Option<Format>,
    hasher: sha2::Sha256,
    image_dir: PathBuf,
    alias_tree: sled::Tree,
    filename_tree: sled::Tree,
    main_tree: sled::Tree,
    db: sled::Db,
}

impl std::fmt::Debug for UploadManager {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("UploadManager").finish()
    }
}

type UploadStream<E> = LocalBoxStream<'static, Result<web::Bytes, E>>;

#[derive(Clone, Debug)]
pub(crate) struct Serde<T> {
    inner: T,
}

impl<T> Serde<T> {
    pub(crate) fn new(inner: T) -> Self {
        Serde { inner }
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

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Details {
    width: usize,
    height: usize,
    content_type: Serde<mime::Mime>,
    created_at: time::OffsetDateTime,
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

struct FilenameIVec {
    inner: sled::IVec,
}

impl FilenameIVec {
    fn new(inner: sled::IVec) -> Self {
        FilenameIVec { inner }
    }
}

impl std::fmt::Debug for FilenameIVec {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", String::from_utf8(self.inner.to_vec()))
    }
}

struct Hash {
    inner: Vec<u8>,
}

impl Hash {
    fn new(inner: Vec<u8>) -> Self {
        Hash { inner }
    }
}

impl std::fmt::Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", base64::encode(&self.inner))
    }
}

enum Dup {
    Exists,
    New,
}

impl Dup {
    fn exists(&self) -> bool {
        matches!(self, Dup::Exists)
    }
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

        Ok(UploadManager {
            inner: Arc::new(UploadManagerInner {
                format,
                hasher: sha2::Sha256::new(),
                image_dir: root_dir,
                alias_tree: db.open_tree("alias")?,
                filename_tree: db.open_tree("filename")?,
                main_tree: db.open_tree("main")?,
                db,
            }),
        })
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
        UploadManagerSession {
            manager: self.clone(),
            alias: None,
            finished: false,
        }
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

impl UploadManagerSession {
    /// Generate a delete token for an alias
    #[instrument(skip(self))]
    pub(crate) async fn delete_token(&self) -> Result<String, Error> {
        let alias = self.alias.clone().ok_or(UploadError::MissingAlias)?;

        debug!("Generating delete token");
        use rand::distributions::{Alphanumeric, Distribution};
        let rng = rand::thread_rng();
        let s: String = Alphanumeric
            .sample_iter(rng)
            .take(10)
            .map(char::from)
            .collect();
        let delete_token = s.clone();

        debug!("Saving delete token");
        let alias_tree = self.manager.inner.alias_tree.clone();
        let key = delete_key(&alias);
        let res = web::block(move || {
            alias_tree.compare_and_swap(
                key.as_bytes(),
                None as Option<sled::IVec>,
                Some(s.as_bytes()),
            )
        })
        .await??;

        if let Err(sled::CompareAndSwapError {
            current: Some(ivec),
            ..
        }) = res
        {
            let s = String::from_utf8(ivec.to_vec())?;

            debug!("Returning existing delete token, {}", s);
            return Ok(s);
        }

        debug!("Returning new delete token, {}", delete_token);
        Ok(delete_token)
    }

    /// Upload the file while preserving the filename, optionally validating the uploaded image
    #[instrument(skip(self, stream))]
    pub(crate) async fn import<E>(
        mut self,
        alias: String,
        content_type: mime::Mime,
        validate: bool,
        mut stream: UploadStream<E>,
    ) -> Result<Self, Error>
    where
        Error: From<E>,
        E: Unpin + 'static,
    {
        let mut bytes_mut = actix_web::web::BytesMut::new();

        debug!("Reading stream to memory");
        while let Some(res) = stream.next().await {
            let bytes = res?;
            bytes_mut.extend_from_slice(&bytes);
        }

        debug!("Validating bytes");
        let (content_type, validated_reader) = crate::validate::validate_image_bytes(
            bytes_mut.freeze(),
            self.manager.inner.format.clone(),
            validate,
        )
        .await?;

        let mut hasher_reader = Hasher::new(validated_reader, self.manager.inner.hasher.clone());

        let tmpfile = crate::tmp_file();
        safe_save_reader(tmpfile.clone(), &mut hasher_reader).await?;
        let hash = hasher_reader.finalize_reset().await?;

        debug!("Storing alias");
        self.alias = Some(alias.clone());
        self.add_existing_alias(&hash, &alias).await?;

        debug!("Saving file");
        self.save_upload(tmpfile, hash, content_type).await?;

        // Return alias to file
        Ok(self)
    }

    /// Upload the file, discarding bytes if it's already present, or saving if it's new
    #[instrument(skip(self, stream))]
    pub(crate) async fn upload<E>(mut self, mut stream: UploadStream<E>) -> Result<Self, Error>
    where
        Error: From<E>,
    {
        let mut bytes_mut = actix_web::web::BytesMut::new();

        debug!("Reading stream to memory");
        while let Some(res) = stream.next().await {
            let bytes = res?;
            bytes_mut.extend_from_slice(&bytes);
        }

        debug!("Validating bytes");
        let (content_type, validated_reader) = crate::validate::validate_image_bytes(
            bytes_mut.freeze(),
            self.manager.inner.format.clone(),
            true,
        )
        .await?;

        let mut hasher_reader = Hasher::new(validated_reader, self.manager.inner.hasher.clone());

        let tmpfile = crate::tmp_file();
        safe_save_reader(tmpfile.clone(), &mut hasher_reader).await?;
        let hash = hasher_reader.finalize_reset().await?;

        debug!("Adding alias");
        self.add_alias(&hash, content_type.clone()).await?;

        debug!("Saving file");
        self.save_upload(tmpfile, hash, content_type).await?;

        // Return alias to file
        Ok(self)
    }

    // check duplicates & store image if new
    async fn save_upload(
        &self,
        tmpfile: PathBuf,
        hash: Hash,
        content_type: mime::Mime,
    ) -> Result<(), Error> {
        let (dup, name) = self.check_duplicate(hash, content_type).await?;

        // bail early with alias to existing file if this is a duplicate
        if dup.exists() {
            debug!("Duplicate exists, not saving file");
            return Ok(());
        }

        // -- WRITE NEW FILE --
        let mut real_path = self.manager.image_dir();
        real_path.push(name);

        crate::safe_move_file(tmpfile, real_path).await?;

        Ok(())
    }

    // check for an already-uploaded image with this hash, returning the path to the target file
    #[instrument(skip(self, hash, content_type))]
    async fn check_duplicate(
        &self,
        hash: Hash,
        content_type: mime::Mime,
    ) -> Result<(Dup, String), Error> {
        let main_tree = self.manager.inner.main_tree.clone();

        let filename = self.next_file(content_type).await?;
        let filename2 = filename.clone();
        let hash2 = hash.inner.clone();
        debug!("Inserting filename for hash");
        let res = web::block(move || {
            main_tree.compare_and_swap(
                hash2,
                None as Option<sled::IVec>,
                Some(filename2.as_bytes()),
            )
        })
        .await??;

        if let Err(sled::CompareAndSwapError {
            current: Some(ivec),
            ..
        }) = res
        {
            let name = String::from_utf8(ivec.to_vec())?;
            debug!("Filename exists for hash, {}", name);
            return Ok((Dup::Exists, name));
        }

        let fname_tree = self.manager.inner.filename_tree.clone();
        let filename2 = filename.clone();
        debug!("Saving filename -> hash relation");
        web::block(move || fname_tree.insert(filename2, hash.inner)).await??;

        Ok((Dup::New, filename))
    }

    // generate a short filename that isn't already in-use
    #[instrument(skip(self, content_type))]
    async fn next_file(&self, content_type: mime::Mime) -> Result<String, Error> {
        let image_dir = self.manager.image_dir();
        use rand::distributions::{Alphanumeric, Distribution};
        let mut limit: usize = 10;
        let mut rng = rand::thread_rng();
        loop {
            debug!("Filename generation loop");
            let mut path = image_dir.clone();
            let s: String = Alphanumeric
                .sample_iter(&mut rng)
                .take(limit)
                .map(char::from)
                .collect();

            let filename = file_name(s, content_type.clone())?;

            path.push(filename.clone());

            if let Err(e) = tokio::fs::metadata(path).await {
                if e.kind() == std::io::ErrorKind::NotFound {
                    debug!("Generated unused filename {}", filename);
                    return Ok(filename);
                }
                return Err(e.into());
            }

            debug!("Filename exists, trying again");

            limit += 1;
        }
    }

    #[instrument(skip(self, hash, alias))]
    async fn add_existing_alias(&self, hash: &Hash, alias: &str) -> Result<(), Error> {
        self.save_alias_hash_mapping(hash, alias).await??;

        self.store_hash_id_alias_mapping(hash, alias).await?;

        Ok(())
    }

    // Add an alias to an existing file
    //
    // This will help if multiple 'users' upload the same file, and one of them wants to delete it
    #[instrument(skip(self, hash, content_type))]
    async fn add_alias(&mut self, hash: &Hash, content_type: mime::Mime) -> Result<(), Error> {
        let alias = self.next_alias(hash, content_type).await?;

        self.store_hash_id_alias_mapping(hash, &alias).await?;

        Ok(())
    }

    // Add a pre-defined alias to an existin file
    //
    // DANGER: this can cause BAD BAD BAD conflicts if the same alias is used for multiple files
    #[instrument(skip(self, hash))]
    async fn store_hash_id_alias_mapping(&self, hash: &Hash, alias: &str) -> Result<(), Error> {
        let alias = alias.to_string();
        loop {
            debug!("hash -> alias save loop");
            let db = self.manager.inner.db.clone();
            let id = web::block(move || db.generate_id()).await??.to_string();

            let alias_tree = self.manager.inner.alias_tree.clone();
            let key = alias_id_key(&alias);
            let id2 = id.clone();
            debug!("Saving alias -> id mapping");
            web::block(move || alias_tree.insert(key.as_bytes(), id2.as_bytes())).await??;

            let key = alias_key(&hash.inner, &id);
            let main_tree = self.manager.inner.main_tree.clone();
            let alias2 = alias.clone();
            debug!("Saving hash/id -> alias mapping");
            let res = web::block(move || {
                main_tree.compare_and_swap(key, None as Option<sled::IVec>, Some(alias2.as_bytes()))
            })
            .await??;

            if res.is_ok() {
                break;
            }

            debug!("Id exists, trying again");
        }

        Ok(())
    }

    // Generate an alias to the file
    #[instrument(skip(self, hash, content_type))]
    async fn next_alias(&mut self, hash: &Hash, content_type: mime::Mime) -> Result<String, Error> {
        use rand::distributions::{Alphanumeric, Distribution};
        let mut limit: usize = 10;
        let mut rng = rand::thread_rng();
        loop {
            debug!("Alias gen loop");
            let s: String = Alphanumeric
                .sample_iter(&mut rng)
                .take(limit)
                .map(char::from)
                .collect();
            let alias = file_name(s, content_type.clone())?;
            self.alias = Some(alias.clone());

            let res = self.save_alias_hash_mapping(hash, &alias).await?;

            if res.is_ok() {
                return Ok(alias);
            }
            debug!("Alias exists, regenning");

            limit += 1;
        }
    }

    // Save an alias to the database
    #[instrument(skip(self, hash))]
    async fn save_alias_hash_mapping(
        &self,
        hash: &Hash,
        alias: &str,
    ) -> Result<Result<(), Error>, Error> {
        let tree = self.manager.inner.alias_tree.clone();
        let vec = hash.inner.clone();
        let alias = alias.to_string();

        debug!("Saving alias -> hash mapping");
        let res = web::block(move || {
            tree.compare_and_swap(alias.as_bytes(), None as Option<sled::IVec>, Some(vec))
        })
        .await??;

        if res.is_err() {
            warn!("Duplicate alias");
            return Ok(Err(UploadError::DuplicateAlias.into()));
        }

        Ok(Ok(()))
    }
}

#[instrument(skip(input))]
pub(crate) async fn safe_save_reader(
    to: PathBuf,
    input: &mut (impl AsyncRead + Unpin),
) -> Result<(), Error> {
    if let Some(path) = to.parent() {
        debug!("Creating directory {:?}", path);
        tokio::fs::create_dir_all(path.to_owned()).await?;
    }

    debug!("Checking if {:?} already exists", to);
    if let Err(e) = tokio::fs::metadata(to.clone()).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    } else {
        return Err(UploadError::FileExists.into());
    }

    debug!("Writing stream to {:?}", to);

    let mut file = crate::file::File::create(to).await?;

    file.write_from_async_read(input).await?;

    Ok(())
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

fn file_name(name: String, content_type: mime::Mime) -> Result<String, Error> {
    Ok(format!("{}{}", name, to_ext(content_type)?))
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

#[cfg(test)]
mod test {
    use super::Hasher;
    use sha2::{Digest, Sha256};
    use std::io::Read;

    macro_rules! test_on_arbiter {
        ($fut:expr) => {
            actix_rt::System::new().block_on(async move {
                let arbiter = actix_rt::Arbiter::new();

                let (tx, rx) = tokio::sync::oneshot::channel();

                arbiter.spawn(async move {
                    let handle = actix_rt::spawn($fut);

                    let _ = tx.send(handle.await.unwrap());
                });

                rx.await.unwrap()
            })
        };
    }

    #[test]
    fn hasher_works() {
        let hash = test_on_arbiter!(async move {
            let file1 = tokio::fs::File::open("./client-examples/earth.gif").await?;

            let mut hasher = Hasher::new(file1, Sha256::new());

            tokio::io::copy(&mut hasher, &mut tokio::io::sink()).await?;

            hasher.finalize_reset().await
        })
        .unwrap();

        let mut file = std::fs::File::open("./client-examples/earth.gif").unwrap();
        let mut vec = Vec::new();
        file.read_to_end(&mut vec).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(vec);
        let correct_hash = hasher.finalize_reset().to_vec();

        assert_eq!(hash.inner, correct_hash);
    }
}
