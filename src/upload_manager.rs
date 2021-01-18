use crate::{
    config::Format,
    error::UploadError,
    migrate::{alias_id_key, alias_key, alias_key_bounds, variant_key_bounds, LatestDb},
    to_ext,
    validate::validate_image,
};
use actix_web::web;
use futures::stream::{Stream, StreamExt};
use sha2::Digest;
use std::{path::PathBuf, pin::Pin, sync::Arc};
use tracing::{debug, error, info, instrument, warn, Span};

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

type UploadStream<E> = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, E>>>>;

#[derive(Clone)]
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

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct Details {
    width: usize,
    height: usize,
    content_type: Serde<mime::Mime>,
    created_at: time::OffsetDateTime,
}

fn mime_from_media_type(media_type: rexiv2::MediaType) -> mime::Mime {
    match media_type {
        rexiv2::MediaType::Jpeg => mime::IMAGE_JPEG,
        rexiv2::MediaType::Png => mime::IMAGE_PNG,
        rexiv2::MediaType::Gif => mime::IMAGE_GIF,
        rexiv2::MediaType::Other(s) if s == "image/webp" => s.parse::<mime::Mime>().unwrap(),
        rexiv2::MediaType::Other(s) if s == "video/mp4" || s == "video/quicktime" => {
            "video/mp4".parse::<mime::Mime>().unwrap()
        }
        _ => mime::APPLICATION_OCTET_STREAM,
    }
}

impl Details {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, UploadError> {
        let metadata = rexiv2::Metadata::new_from_buffer(bytes)?;
        let mime_type = mime_from_media_type(metadata.get_media_type()?);
        let width = metadata.get_pixel_width();
        let height = metadata.get_pixel_height();
        let details = Details::now(width as usize, height as usize, mime_type);
        Ok(details)
    }

    pub(crate) async fn from_path(path: PathBuf) -> Result<Self, UploadError> {
        let (mime_type, width, height) = web::block(move || {
            rexiv2::Metadata::new_from_path(&path).and_then(|metadata| {
                metadata
                    .get_media_type()
                    .map(mime_from_media_type)
                    .map(|mime_type| {
                        (
                            mime_type,
                            metadata.get_pixel_width(),
                            metadata.get_pixel_height(),
                        )
                    })
            })
        })
        .await?;

        Ok(Details::now(width as usize, height as usize, mime_type))
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
    pub(crate) async fn new(
        mut root_dir: PathBuf,
        format: Option<Format>,
    ) -> Result<Self, UploadError> {
        let root_clone = root_dir.clone();
        // sled automatically creates it's own directories
        let db = web::block(move || LatestDb::exists(root_clone).migrate()).await?;

        root_dir.push("files");

        // Ensure file dir exists
        async_fs::create_dir_all(root_dir.clone()).await?;

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
    pub(crate) async fn store_variant(
        &self,
        path: PathBuf,
        filename: String,
    ) -> Result<(), UploadError> {
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Getting hash");
        let hash: sled::IVec = web::block(move || fname_tree.get(filename.as_bytes()))
            .await?
            .ok_or(UploadError::MissingFilename)?;

        let key = variant_key(&hash, &path_string);
        let main_tree = self.inner.main_tree.clone();
        debug!("Storing variant");
        web::block(move || main_tree.insert(key, path_string.as_bytes())).await?;
        debug!("Stored variant");

        Ok(())
    }

    /// Get the image details for a given variant
    pub(crate) async fn variant_details(
        &self,
        path: PathBuf,
        filename: String,
    ) -> Result<Option<Details>, UploadError> {
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Getting hash");
        let hash: sled::IVec = web::block(move || fname_tree.get(filename.as_bytes()))
            .await?
            .ok_or(UploadError::MissingFilename)?;

        let key = variant_details_key(&hash, &path_string);
        let main_tree = self.inner.main_tree.clone();
        debug!("Getting details");
        let opt = match web::block(move || main_tree.get(key)).await? {
            Some(ivec) => match serde_json::from_slice(&ivec) {
                Ok(details) => Some(details),
                Err(_) => None,
            },
            None => None,
        };
        debug!("Got details");

        Ok(opt)
    }

    pub(crate) async fn store_variant_details(
        &self,
        path: PathBuf,
        filename: String,
        details: &Details,
    ) -> Result<(), UploadError> {
        let path_string = path.to_str().ok_or(UploadError::Path)?.to_string();

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Getting hash");
        let hash: sled::IVec = web::block(move || fname_tree.get(filename.as_bytes()))
            .await?
            .ok_or(UploadError::MissingFilename)?;

        let key = variant_details_key(&hash, &path_string);
        let main_tree = self.inner.main_tree.clone();
        let details_value = serde_json::to_string(details)?;
        debug!("Storing details");
        web::block(move || main_tree.insert(key, details_value.as_bytes())).await?;
        debug!("Stored details");

        Ok(())
    }

    /// Get a list of aliases for a given file
    pub(crate) async fn aliases_by_filename(
        &self,
        filename: String,
    ) -> Result<Vec<String>, UploadError> {
        let fname_tree = self.inner.filename_tree.clone();
        let hash = web::block(move || fname_tree.get(filename.as_bytes()))
            .await?
            .ok_or(UploadError::MissingAlias)?;

        self.aliases_by_hash(&hash).await
    }

    /// Get a list of aliases for a given alias
    pub(crate) async fn aliases_by_alias(&self, alias: String) -> Result<Vec<String>, UploadError> {
        let alias_tree = self.inner.alias_tree.clone();
        let hash = web::block(move || alias_tree.get(alias.as_bytes()))
            .await?
            .ok_or(UploadError::MissingFilename)?;

        self.aliases_by_hash(&hash).await
    }

    async fn aliases_by_hash(&self, hash: &sled::IVec) -> Result<Vec<String>, UploadError> {
        let (start, end) = alias_key_bounds(hash);
        let main_tree = self.inner.main_tree.clone();
        let aliases = web::block(move || {
            main_tree
                .range(start..end)
                .values()
                .collect::<Result<Vec<_>, _>>()
        })
        .await?;

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
    pub(crate) async fn delete_without_token(&self, alias: String) -> Result<(), UploadError> {
        let token_key = delete_key(&alias);
        let alias_tree = self.inner.alias_tree.clone();
        let token = web::block(move || alias_tree.get(token_key.as_bytes()))
            .await?
            .ok_or(UploadError::MissingAlias)?;

        self.delete(alias, String::from_utf8(token.to_vec())?).await
    }

    /// Delete the alias, and the file & variants if no more aliases exist
    #[instrument(skip(self, alias, token))]
    pub(crate) async fn delete(&self, alias: String, token: String) -> Result<(), UploadError> {
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
                let id = String::from_utf8(id.to_vec()).map_err(|e| trans_err(e.into()))?;

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
        .await?;

        // -- CHECK IF ANY OTHER ALIASES EXIST --
        let main_tree = self.inner.main_tree.clone();
        let (start, end) = alias_key_bounds(&hash);
        debug!("Checking for additional aliases referencing hash");
        let any_aliases = web::block(move || {
            Ok(main_tree.range(start..end).next().is_some()) as Result<bool, UploadError>
        })
        .await?;

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
            .await?
            .ok_or(UploadError::MissingFile)?;

        // -- DELETE FILES --
        let this = self.clone();
        debug!("Spawning cleanup task");
        let span = Span::current();
        actix_rt::spawn(async move {
            let entered = span.enter();
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
            drop(entered);
        });

        Ok(())
    }

    /// Generate a delete token for an alias
    #[instrument(skip(self))]
    pub(crate) async fn delete_token(&self, alias: String) -> Result<String, UploadError> {
        debug!("Generating delete token");
        use rand::distributions::{Alphanumeric, Distribution};
        let rng = rand::thread_rng();
        let s: String = Alphanumeric.sample_iter(rng).take(10).collect();
        let delete_token = s.clone();

        debug!("Saving delete token");
        let alias_tree = self.inner.alias_tree.clone();
        let key = delete_key(&alias);
        let res = web::block(move || {
            alias_tree.compare_and_swap(
                key.as_bytes(),
                None as Option<sled::IVec>,
                Some(s.as_bytes()),
            )
        })
        .await?;

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
        &self,
        alias: String,
        content_type: mime::Mime,
        validate: bool,
        stream: UploadStream<E>,
    ) -> Result<String, UploadError>
    where
        UploadError: From<E>,
        E: Unpin,
    {
        // -- READ IN BYTES FROM CLIENT --
        debug!("Reading stream");
        let tmpfile = crate::tmp_file();
        safe_save_stream(tmpfile.clone(), stream).await?;

        let content_type = if validate {
            debug!("Validating image");
            let format = self.inner.format.clone();
            validate_image(tmpfile.clone(), format).await?
        } else {
            content_type
        };

        // -- DUPLICATE CHECKS --

        // Cloning bytes is fine because it's actually a pointer
        debug!("Hashing bytes");
        let hash = self.hash(tmpfile.clone()).await?;

        debug!("Storing alias");
        self.add_existing_alias(&hash, &alias).await?;

        debug!("Saving file");
        self.save_upload(tmpfile, hash, content_type).await?;

        // Return alias to file
        Ok(alias)
    }

    /// Upload the file, discarding bytes if it's already present, or saving if it's new
    #[instrument(skip(self, stream))]
    pub(crate) async fn upload<E>(&self, stream: UploadStream<E>) -> Result<String, UploadError>
    where
        UploadError: From<E>,
        E: Unpin,
    {
        // -- READ IN BYTES FROM CLIENT --
        debug!("Reading stream");
        let tmpfile = crate::tmp_file();
        safe_save_stream(tmpfile.clone(), stream).await?;

        // -- VALIDATE IMAGE --
        debug!("Validating image");
        let format = self.inner.format.clone();
        let content_type = validate_image(tmpfile.clone(), format).await?;

        // -- DUPLICATE CHECKS --

        // Cloning bytes is fine because it's actually a pointer
        debug!("Hashing bytes");
        let hash = self.hash(tmpfile.clone()).await?;

        debug!("Adding alias");
        let alias = self.add_alias(&hash, content_type.clone()).await?;

        debug!("Saving file");
        self.save_upload(tmpfile, hash, content_type).await?;

        // Return alias to file
        Ok(alias)
    }

    /// Fetch the real on-disk filename given an alias
    #[instrument(skip(self))]
    pub(crate) async fn from_alias(&self, alias: String) -> Result<String, UploadError> {
        let tree = self.inner.alias_tree.clone();
        debug!("Getting hash from alias");
        let hash = web::block(move || tree.get(alias.as_bytes()))
            .await?
            .ok_or(UploadError::MissingAlias)?;

        let main_tree = self.inner.main_tree.clone();
        debug!("Getting filename from hash");
        let filename = web::block(move || main_tree.get(hash))
            .await?
            .ok_or(UploadError::MissingFile)?;

        let filename = String::from_utf8(filename.to_vec())?;

        Ok(filename)
    }

    // Find image variants and remove them from the DB and the disk
    #[instrument(skip(self))]
    async fn cleanup_files(&self, filename: FilenameIVec) -> Result<(), UploadError> {
        let filename = filename.inner;
        let mut path = self.image_dir();
        let fname = String::from_utf8(filename.to_vec())?;
        path.push(fname);

        let mut errors = Vec::new();
        debug!("Deleting {:?}", path);
        if let Err(e) = async_fs::remove_file(path).await {
            errors.push(e.into());
        }

        let fname_tree = self.inner.filename_tree.clone();
        debug!("Deleting filename -> hash mapping");
        let hash = web::block(move || fname_tree.remove(filename))
            .await?
            .ok_or(UploadError::MissingFile)?;

        let (start, end) = variant_key_bounds(&hash);
        let main_tree = self.inner.main_tree.clone();
        debug!("Fetching file variants");
        let keys = web::block(move || {
            let mut keys = Vec::new();
            for key in main_tree.range(start..end).keys() {
                keys.push(key?.to_owned());
            }

            Ok(keys) as Result<Vec<sled::IVec>, UploadError>
        })
        .await?;

        debug!("{} files prepared for deletion", keys.len());

        for key in keys {
            let main_tree = self.inner.main_tree.clone();
            if let Some(path) = web::block(move || main_tree.remove(key)).await? {
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

    // check duplicates & store image if new
    async fn save_upload(
        &self,
        tmpfile: PathBuf,
        hash: Hash,
        content_type: mime::Mime,
    ) -> Result<(), UploadError> {
        let (dup, name) = self.check_duplicate(hash, content_type).await?;

        // bail early with alias to existing file if this is a duplicate
        if dup.exists() {
            debug!("Duplicate exists, not saving file");
            return Ok(());
        }

        // -- WRITE NEW FILE --
        let mut real_path = self.image_dir();
        real_path.push(name);

        crate::safe_move_file(tmpfile, real_path).await?;

        Ok(())
    }

    // produce a sh256sum of the uploaded file
    async fn hash(&self, tmpfile: PathBuf) -> Result<Hash, UploadError> {
        let mut hasher = self.inner.hasher.clone();

        let file = async_fs::File::open(tmpfile).await?;
        let mut stream = Box::pin(crate::read_to_stream(file));

        while let Some(res) = stream.next().await {
            let bytes = res?;
            hasher = web::block(move || {
                hasher.update(&bytes);
                Ok(hasher) as Result<_, UploadError>
            })
            .await?;
        }

        let hash =
            web::block(move || Ok(hasher.finalize_reset().to_vec()) as Result<_, UploadError>)
                .await?;

        Ok(Hash::new(hash))
    }

    // check for an already-uploaded image with this hash, returning the path to the target file
    #[instrument(skip(self, hash, content_type))]
    async fn check_duplicate(
        &self,
        hash: Hash,
        content_type: mime::Mime,
    ) -> Result<(Dup, String), UploadError> {
        let main_tree = self.inner.main_tree.clone();

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
        .await?;

        if let Err(sled::CompareAndSwapError {
            current: Some(ivec),
            ..
        }) = res
        {
            let name = String::from_utf8(ivec.to_vec())?;
            debug!("Filename exists for hash, {}", name);
            return Ok((Dup::Exists, name));
        }

        let fname_tree = self.inner.filename_tree.clone();
        let filename2 = filename.clone();
        debug!("Saving filename -> hash relation");
        web::block(move || fname_tree.insert(filename2, hash.inner)).await?;

        Ok((Dup::New, filename))
    }

    // generate a short filename that isn't already in-use
    #[instrument(skip(self, content_type))]
    async fn next_file(&self, content_type: mime::Mime) -> Result<String, UploadError> {
        let image_dir = self.image_dir();
        use rand::distributions::{Alphanumeric, Distribution};
        let mut limit: usize = 10;
        let rng = rand::thread_rng();
        loop {
            debug!("Filename generation loop");
            let mut path = image_dir.clone();
            let s: String = Alphanumeric.sample_iter(rng).take(limit).collect();

            let filename = file_name(s, content_type.clone())?;

            path.push(filename.clone());

            if let Err(e) = async_fs::metadata(path).await {
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
    async fn add_existing_alias(&self, hash: &Hash, alias: &str) -> Result<(), UploadError> {
        self.save_alias(hash, alias).await??;

        self.store_alias(hash, alias).await?;

        Ok(())
    }

    // Add an alias to an existing file
    //
    // This will help if multiple 'users' upload the same file, and one of them wants to delete it
    #[instrument(skip(self, hash, content_type))]
    async fn add_alias(
        &self,
        hash: &Hash,
        content_type: mime::Mime,
    ) -> Result<String, UploadError> {
        let alias = self.next_alias(hash, content_type).await?;

        self.store_alias(hash, &alias).await?;

        Ok(alias)
    }

    // Add a pre-defined alias to an existin file
    //
    // DANGER: this can cause BAD BAD BAD conflicts if the same alias is used for multiple files
    #[instrument(skip(self, hash))]
    async fn store_alias(&self, hash: &Hash, alias: &str) -> Result<(), UploadError> {
        let alias = alias.to_string();
        loop {
            debug!("hash -> alias save loop");
            let db = self.inner.db.clone();
            let id = web::block(move || db.generate_id()).await?.to_string();

            let key = alias_key(&hash.inner, &id);
            let main_tree = self.inner.main_tree.clone();
            let alias2 = alias.clone();
            debug!("Saving hash/id -> alias mapping");
            let res = web::block(move || {
                main_tree.compare_and_swap(key, None as Option<sled::IVec>, Some(alias2.as_bytes()))
            })
            .await?;

            if res.is_ok() {
                let alias_tree = self.inner.alias_tree.clone();
                let key = alias_id_key(&alias);
                debug!("Saving alias -> id mapping");
                web::block(move || alias_tree.insert(key.as_bytes(), id.as_bytes())).await?;

                break;
            }

            debug!("Id exists, trying again");
        }

        Ok(())
    }

    // Generate an alias to the file
    #[instrument(skip(self, hash, content_type))]
    async fn next_alias(
        &self,
        hash: &Hash,
        content_type: mime::Mime,
    ) -> Result<String, UploadError> {
        use rand::distributions::{Alphanumeric, Distribution};
        let mut limit: usize = 10;
        let rng = rand::thread_rng();
        loop {
            debug!("Alias gen loop");
            let s: String = Alphanumeric.sample_iter(rng).take(limit).collect();
            let alias = file_name(s, content_type.clone())?;

            let res = self.save_alias(hash, &alias).await?;

            if res.is_ok() {
                return Ok(alias);
            }
            debug!("Alias exists, regenning");

            limit += 1;
        }
    }

    // Save an alias to the database
    #[instrument(skip(self, hash))]
    async fn save_alias(
        &self,
        hash: &Hash,
        alias: &str,
    ) -> Result<Result<(), UploadError>, UploadError> {
        let tree = self.inner.alias_tree.clone();
        let vec = hash.inner.clone();
        let alias = alias.to_string();

        debug!("Saving alias");
        let res = web::block(move || {
            tree.compare_and_swap(alias.as_bytes(), None as Option<sled::IVec>, Some(vec))
        })
        .await?;

        if res.is_err() {
            warn!("Duplicate alias");
            return Ok(Err(UploadError::DuplicateAlias));
        }

        Ok(Ok(()))
    }
}

#[instrument(skip(stream))]
async fn safe_save_stream<E>(to: PathBuf, mut stream: UploadStream<E>) -> Result<(), UploadError>
where
    UploadError: From<E>,
    E: Unpin,
{
    if let Some(path) = to.parent() {
        debug!("Creating directory {:?}", path);
        async_fs::create_dir_all(path.to_owned()).await?;
    }

    debug!("Checking if {:?} already exists", to);
    if let Err(e) = async_fs::metadata(to.clone()).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    } else {
        return Err(UploadError::FileExists);
    }

    debug!("Writing stream to {:?}", to);

    let mut file = async_fs::File::create(to).await?;

    use futures_lite::AsyncWriteExt;
    while let Some(res) = stream.next().await {
        let bytes = res?;
        file.write_all(&bytes).await?;
    }
    file.flush().await?;

    Ok(())
}

async fn remove_path(path: sled::IVec) -> Result<(), UploadError> {
    let path_string = String::from_utf8(path.to_vec())?;
    async_fs::remove_file(path_string).await?;
    Ok(())
}

fn trans_err(e: UploadError) -> sled::transaction::ConflictableTransactionError<UploadError> {
    sled::transaction::ConflictableTransactionError::Abort(e)
}

fn file_name(name: String, content_type: mime::Mime) -> Result<String, UploadError> {
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
