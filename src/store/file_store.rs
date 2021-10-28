use crate::{file::File, store::Store};
use actix_web::web::Bytes;
use futures_util::stream::Stream;
use std::{
    path::{Path, PathBuf},
    pin::Pin,
};
use storage_path_generator::Generator;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, error, instrument};
use uuid::Uuid;

mod file_id;
mod restructure;
pub(crate) use file_id::FileId;

// - Settings Tree
//   - last-path -> last generated path
//   - fs-restructure-01-complete -> bool

const GENERATOR_KEY: &[u8] = b"last-path";

#[derive(Debug, thiserror::Error)]
pub(crate) enum FileError {
    #[error(transparent)]
    Sled(#[from] sled::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    PathGenerator(#[from] storage_path_generator::PathError),

    #[error("Error formatting file store identifier")]
    IdError,

    #[error("Mailformed file store identifier")]
    PrefixError,

    #[error("Tried to save over existing file")]
    FileExists,
}

#[derive(Clone)]
pub(crate) struct FileStore {
    path_gen: Generator,
    root_dir: PathBuf,
    settings_tree: sled::Tree,
}

#[async_trait::async_trait(?Send)]
impl Store for FileStore {
    type Error = FileError;
    type Identifier = FileId;
    type Stream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>>>>;

    async fn save_async_read<Reader>(
        &self,
        reader: &mut Reader,
    ) -> Result<Self::Identifier, Self::Error>
    where
        Reader: AsyncRead + Unpin,
    {
        let path = self.next_file()?;

        if let Err(e) = self.safe_save_reader(&path, reader).await {
            self.safe_remove_file(&path).await?;
            return Err(e);
        }

        self.file_id_from_path(path)
    }

    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, Self::Error> {
        let path = self.next_file()?;

        if let Err(e) = self.safe_save_bytes(&path, bytes).await {
            self.safe_remove_file(&path).await?;
            return Err(e);
        }

        self.file_id_from_path(path)
    }

    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, Self::Error> {
        let path = self.path_from_file_id(identifier);

        let stream = File::open(path)
            .await?
            .read_to_stream(from_start, len)
            .await?;

        Ok(Box::pin(stream))
    }

    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Unpin,
    {
        let path = self.path_from_file_id(identifier);

        File::open(&path).await?.read_to_async_write(writer).await?;

        Ok(())
    }

    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Self::Error> {
        let path = self.path_from_file_id(identifier);

        let len = tokio::fs::metadata(path).await?.len();

        Ok(len)
    }

    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Self::Error> {
        let path = self.path_from_file_id(identifier);

        self.safe_remove_file(path).await?;

        Ok(())
    }
}

impl FileStore {
    pub fn build(root_dir: PathBuf, db: &sled::Db) -> Result<Self, FileError> {
        let settings_tree = db.open_tree("settings")?;

        let path_gen = init_generator(&settings_tree)?;

        Ok(FileStore {
            root_dir,
            path_gen,
            settings_tree,
        })
    }

    fn next_directory(&self) -> Result<PathBuf, FileError> {
        let path = self.path_gen.next();

        self.settings_tree
            .insert(GENERATOR_KEY, path.to_be_bytes())?;

        let mut target_path = self.root_dir.join("files");
        for dir in path.to_strings() {
            target_path.push(dir)
        }

        Ok(target_path)
    }

    fn next_file(&self) -> Result<PathBuf, FileError> {
        let target_path = self.next_directory()?;

        let filename = Uuid::new_v4().to_string();

        Ok(target_path.join(filename))
    }

    async fn safe_remove_file<P: AsRef<Path>>(&self, path: P) -> Result<(), FileError> {
        tokio::fs::remove_file(&path).await?;
        self.try_remove_parents(path.as_ref()).await;
        Ok(())
    }

    async fn try_remove_parents(&self, mut path: &Path) {
        while let Some(parent) = path.parent() {
            if parent.ends_with(&self.root_dir) {
                return;
            }

            if tokio::fs::remove_dir(parent).await.is_err() {
                return;
            }

            path = parent;
        }
    }

    // Try writing to a file
    #[instrument(name = "Saving file", skip(bytes), fields(path = tracing::field::debug(&path.as_ref())))]
    async fn safe_save_bytes<P: AsRef<Path>>(
        &self,
        path: P,
        bytes: Bytes,
    ) -> Result<(), FileError> {
        safe_create_parent(&path).await?;

        // Only write the file if it doesn't already exist
        debug!("Checking if {:?} already exists", path.as_ref());
        if let Err(e) = tokio::fs::metadata(&path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        } else {
            return Ok(());
        }

        // Open the file for writing
        debug!("Creating {:?}", path.as_ref());
        let mut file = File::create(&path).await?;

        // try writing
        debug!("Writing to {:?}", path.as_ref());
        if let Err(e) = file.write_from_bytes(bytes).await {
            error!("Error writing {:?}, {}", path.as_ref(), e);
            // remove file if writing failed before completion
            self.safe_remove_file(path).await?;
            return Err(e.into());
        }
        debug!("{:?} written", path.as_ref());

        Ok(())
    }

    #[instrument(skip(input), fields(to = tracing::field::debug(&to.as_ref())))]
    async fn safe_save_reader<P: AsRef<Path>>(
        &self,
        to: P,
        input: &mut (impl AsyncRead + Unpin + ?Sized),
    ) -> Result<(), FileError> {
        safe_create_parent(&to).await?;

        debug!("Checking if {:?} already exists", to.as_ref());
        if let Err(e) = tokio::fs::metadata(&to).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        } else {
            return Err(FileError::FileExists);
        }

        debug!("Writing stream to {:?}", to.as_ref());

        let mut file = File::create(to).await?;

        file.write_from_async_read(input).await?;

        Ok(())
    }

    // try moving a file
    #[instrument(name = "Moving file", fields(from = tracing::field::debug(&from.as_ref()), to = tracing::field::debug(&to.as_ref())))]
    pub(crate) async fn safe_move_file<P: AsRef<Path>, Q: AsRef<Path>>(
        &self,
        from: P,
        to: Q,
    ) -> Result<(), FileError> {
        safe_create_parent(&to).await?;

        debug!("Checking if {:?} already exists", to.as_ref());
        if let Err(e) = tokio::fs::metadata(&to).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        } else {
            return Err(FileError::FileExists);
        }

        debug!("Moving {:?} to {:?}", from.as_ref(), to.as_ref());
        tokio::fs::copy(&from, &to).await?;
        self.safe_remove_file(from).await?;
        Ok(())
    }
}

pub(crate) async fn safe_create_parent<P: AsRef<Path>>(path: P) -> Result<(), FileError> {
    if let Some(path) = path.as_ref().parent() {
        debug!("Creating directory {:?}", path);
        tokio::fs::create_dir_all(path).await?;
    }

    Ok(())
}

fn init_generator(settings: &sled::Tree) -> Result<Generator, FileError> {
    if let Some(ivec) = settings.get(GENERATOR_KEY)? {
        Ok(Generator::from_existing(
            storage_path_generator::Path::from_be_bytes(ivec.to_vec())?,
        ))
    } else {
        Ok(Generator::new())
    }
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore")
            .field("path_gen", &self.path_gen)
            .field("root_dir", &self.root_dir)
            .finish()
    }
}
