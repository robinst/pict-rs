use crate::{
    file::File,
    repo::{Repo, SettingsRepo},
    store::{Store, StoreConfig},
};
use actix_web::web::Bytes;
use futures_util::stream::Stream;
use std::{
    path::{Path, PathBuf},
    pin::Pin,
};
use storage_path_generator::Generator;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::StreamReader;
use tracing::Instrument;

mod file_id;
pub(crate) use file_id::FileId;

use super::StoreError;

// - Settings Tree
//   - last-path -> last generated path

const GENERATOR_KEY: &str = "last-path";

#[derive(Debug, thiserror::Error)]
pub(crate) enum FileError {
    #[error("Failed to read or write file")]
    Io(#[from] std::io::Error),

    #[error("Failed to generate path")]
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
    repo: Repo,
}

impl StoreConfig for FileStore {
    type Store = FileStore;

    fn build(self) -> Self::Store {
        self
    }
}

#[async_trait::async_trait(?Send)]
impl Store for FileStore {
    type Identifier = FileId;
    type Stream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>>>>;

    #[tracing::instrument(skip(reader))]
    async fn save_async_read<Reader>(
        &self,
        mut reader: Reader,
    ) -> Result<Self::Identifier, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        let path = self.next_file().await?;

        if let Err(e) = self.safe_save_reader(&path, &mut reader).await {
            self.safe_remove_file(&path).await?;
            return Err(e.into());
        }

        Ok(self.file_id_from_path(path)?)
    }

    async fn save_stream<S>(&self, stream: S) -> Result<Self::Identifier, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        self.save_async_read(StreamReader::new(stream)).await
    }

    #[tracing::instrument(skip(bytes))]
    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, StoreError> {
        let path = self.next_file().await?;

        if let Err(e) = self.safe_save_bytes(&path, bytes).await {
            self.safe_remove_file(&path).await?;
            return Err(e.into());
        }

        Ok(self.file_id_from_path(path)?)
    }

    #[tracing::instrument]
    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, StoreError> {
        let path = self.path_from_file_id(identifier);

        let file_span = tracing::trace_span!(parent: None, "File Stream");
        let file = file_span
            .in_scope(|| File::open(path))
            .instrument(file_span.clone())
            .await
            .map_err(FileError::from)?;

        let stream = file_span
            .in_scope(|| file.read_to_stream(from_start, len))
            .instrument(file_span)
            .await?;

        Ok(Box::pin(stream))
    }

    #[tracing::instrument(skip(writer))]
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

    #[tracing::instrument]
    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, StoreError> {
        let path = self.path_from_file_id(identifier);

        let len = tokio::fs::metadata(path)
            .await
            .map_err(FileError::from)?
            .len();

        Ok(len)
    }

    #[tracing::instrument]
    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), StoreError> {
        let path = self.path_from_file_id(identifier);

        self.safe_remove_file(path).await?;

        Ok(())
    }
}

impl FileStore {
    pub(crate) async fn build(root_dir: PathBuf, repo: Repo) -> Result<Self, StoreError> {
        let path_gen = init_generator(&repo).await?;

        Ok(FileStore {
            root_dir,
            path_gen,
            repo,
        })
    }

    async fn next_directory(&self) -> Result<PathBuf, StoreError> {
        let path = self.path_gen.next();

        match self.repo {
            Repo::Sled(ref sled_repo) => {
                sled_repo
                    .set(GENERATOR_KEY, path.to_be_bytes().into())
                    .await?;
            }
        }

        let mut target_path = self.root_dir.clone();
        for dir in path.to_strings() {
            target_path.push(dir)
        }

        Ok(target_path)
    }

    async fn next_file(&self) -> Result<PathBuf, StoreError> {
        let target_path = self.next_directory().await?;
        let filename = uuid::Uuid::new_v4().to_string();

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
    async fn safe_save_bytes<P: AsRef<Path>>(
        &self,
        path: P,
        bytes: Bytes,
    ) -> Result<(), FileError> {
        safe_create_parent(&path).await?;

        // Only write the file if it doesn't already exist
        if let Err(e) = tokio::fs::metadata(&path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        } else {
            return Ok(());
        }

        // Open the file for writing
        let mut file = File::create(&path).await?;

        // try writing
        if let Err(e) = file.write_from_bytes(bytes).await {
            // remove file if writing failed before completion
            self.safe_remove_file(path).await?;
            return Err(e.into());
        }

        Ok(())
    }

    async fn safe_save_reader<P: AsRef<Path>>(
        &self,
        to: P,
        input: &mut (impl AsyncRead + Unpin + ?Sized),
    ) -> Result<(), FileError> {
        safe_create_parent(&to).await?;

        if let Err(e) = tokio::fs::metadata(&to).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        } else {
            return Err(FileError::FileExists);
        }

        let mut file = File::create(to).await?;

        file.write_from_async_read(input).await?;

        Ok(())
    }
}

pub(crate) async fn safe_create_parent<P: AsRef<Path>>(path: P) -> Result<(), FileError> {
    if let Some(path) = path.as_ref().parent() {
        tokio::fs::create_dir_all(path).await?;
    }

    Ok(())
}

async fn init_generator(repo: &Repo) -> Result<Generator, StoreError> {
    match repo {
        Repo::Sled(sled_repo) => {
            if let Some(ivec) = sled_repo.get(GENERATOR_KEY).await? {
                Ok(Generator::from_existing(
                    storage_path_generator::Path::from_be_bytes(ivec.to_vec())
                        .map_err(FileError::from)?,
                ))
            } else {
                Ok(Generator::new())
            }
        }
    }
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore")
            .field("path_gen", &"generator")
            .field("root_dir", &self.root_dir)
            .finish()
    }
}
