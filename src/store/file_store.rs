use crate::{
    error_code::ErrorCode, file::File, repo::ArcRepo, store::Store, stream::LocalBoxStream,
};
use actix_web::web::Bytes;
use futures_core::Stream;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use storage_path_generator::Generator;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::io::StreamReader;
use tracing::Instrument;

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

    #[error("Couldn't convert Path to String")]
    StringError,

    #[error("Tried to save over existing file")]
    FileExists,
}

impl FileError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Io(_) => ErrorCode::FILE_IO_ERROR,
            Self::PathGenerator(_) => ErrorCode::PARSE_PATH_ERROR,
            Self::FileExists => ErrorCode::FILE_EXISTS,
            Self::StringError => ErrorCode::FORMAT_FILE_ID_ERROR,
        }
    }
}

#[derive(Clone)]
pub(crate) struct FileStore {
    path_gen: Generator,
    root_dir: PathBuf,
    repo: ArcRepo,
}

#[async_trait::async_trait(?Send)]
impl Store for FileStore {
    async fn health_check(&self) -> Result<(), StoreError> {
        tokio::fs::metadata(&self.root_dir)
            .await
            .map_err(FileError::from)?;

        Ok(())
    }

    #[tracing::instrument(skip(reader))]
    async fn save_async_read<Reader>(
        &self,
        mut reader: Reader,
        _content_type: mime::Mime,
    ) -> Result<Arc<str>, StoreError>
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

    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        self.save_async_read(StreamReader::new(stream), content_type)
            .await
    }

    #[tracing::instrument(skip(bytes))]
    async fn save_bytes(
        &self,
        bytes: Bytes,
        _content_type: mime::Mime,
    ) -> Result<Arc<str>, StoreError> {
        let path = self.next_file().await?;

        if let Err(e) = self.safe_save_bytes(&path, bytes).await {
            self.safe_remove_file(&path).await?;
            return Err(e.into());
        }

        Ok(self.file_id_from_path(path)?)
    }

    fn public_url(&self, _identifier: &Arc<str>) -> Option<url::Url> {
        None
    }

    #[tracing::instrument]
    async fn to_stream(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<LocalBoxStream<'static, std::io::Result<Bytes>>, StoreError> {
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
        identifier: &Arc<str>,
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
    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError> {
        let path = self.path_from_file_id(identifier);

        let len = tokio::fs::metadata(path)
            .await
            .map_err(FileError::from)?
            .len();

        Ok(len)
    }

    #[tracing::instrument]
    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError> {
        let path = self.path_from_file_id(identifier);

        self.safe_remove_file(path).await?;

        Ok(())
    }
}

impl FileStore {
    #[tracing::instrument(skip(repo))]
    pub(crate) async fn build(root_dir: PathBuf, repo: ArcRepo) -> color_eyre::Result<Self> {
        let path_gen = init_generator(&repo).await?;

        tokio::fs::create_dir_all(&root_dir).await?;

        Ok(FileStore {
            root_dir,
            path_gen,
            repo,
        })
    }

    fn file_id_from_path(&self, path: PathBuf) -> Result<Arc<str>, FileError> {
        path.to_str().ok_or(FileError::StringError).map(Into::into)
    }

    fn path_from_file_id(&self, file_id: &Arc<str>) -> PathBuf {
        self.root_dir.join(file_id.as_ref())
    }

    async fn next_directory(&self) -> Result<PathBuf, StoreError> {
        let path = self.path_gen.next();

        self.repo
            .set(GENERATOR_KEY, path.to_be_bytes().into())
            .await?;

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

async fn init_generator(repo: &ArcRepo) -> Result<Generator, StoreError> {
    if let Some(ivec) = repo.get(GENERATOR_KEY).await? {
        Ok(Generator::from_existing(
            storage_path_generator::Path::from_be_bytes(ivec.to_vec()).map_err(FileError::from)?,
        ))
    } else {
        Ok(Generator::new())
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
