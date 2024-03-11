use crate::{
    error_code::ErrorCode, file::File, future::WithPollTimer, store::Store, stream::LocalBoxStream,
};
use actix_web::web::Bytes;
use futures_core::Stream;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::Instrument;

use super::StoreError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum FileError {
    #[error("Failed to read or write file")]
    Io(#[from] std::io::Error),

    #[error("Couldn't strip root dir")]
    PrefixError,

    #[error("Couldn't convert Path to String")]
    StringError,

    #[error("Tried to save over existing file")]
    FileExists,
}

impl FileError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Io(_) => ErrorCode::FILE_IO_ERROR,
            Self::FileExists => ErrorCode::FILE_EXISTS,
            Self::StringError | Self::PrefixError => ErrorCode::FORMAT_FILE_ID_ERROR,
        }
    }
}

#[derive(Clone)]
pub(crate) struct FileStore {
    root_dir: PathBuf,
}

impl Store for FileStore {
    #[tracing::instrument(level = "debug", skip(self))]
    async fn health_check(&self) -> Result<(), StoreError> {
        tokio::fs::metadata(&self.root_dir)
            .await
            .map_err(FileError::from)?;

        Ok(())
    }

    async fn save_stream<S>(
        &self,
        stream: S,
        _content_type: mime::Mime,
        extension: Option<&str>,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>>,
    {
        let path = self.next_file(extension);

        if let Err(e) = self
            .safe_save_stream(&path, crate::stream::error_injector(stream))
            .await
        {
            self.safe_remove_file(&path).await?;
            return Err(e.into());
        }

        Ok(self.file_id_from_path(path)?)
    }

    fn public_url(&self, _identifier: &Arc<str>) -> Option<url::Url> {
        None
    }

    #[tracing::instrument(skip(self))]
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

        Ok(Box::pin(crate::stream::error_injector(stream)))
    }

    #[tracing::instrument(skip(self))]
    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError> {
        let path = self.path_from_file_id(identifier);

        let len = tokio::fs::metadata(path)
            .await
            .map_err(FileError::from)?
            .len();

        Ok(len)
    }

    #[tracing::instrument(skip(self))]
    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError> {
        let path = self.path_from_file_id(identifier);

        self.safe_remove_file(path).await?;

        Ok(())
    }
}

impl FileStore {
    pub(crate) async fn build(root_dir: PathBuf) -> color_eyre::Result<Self> {
        tokio::fs::create_dir_all(&root_dir).await?;

        Ok(FileStore { root_dir })
    }

    fn file_id_from_path(&self, path: PathBuf) -> Result<Arc<str>, FileError> {
        path.strip_prefix(&self.root_dir)
            .map_err(|_| FileError::PrefixError)?
            .to_str()
            .ok_or(FileError::StringError)
            .map(Into::into)
    }

    fn path_from_file_id(&self, file_id: &Arc<str>) -> PathBuf {
        self.root_dir.join(file_id.as_ref())
    }

    fn next_file(&self, extension: Option<&str>) -> PathBuf {
        crate::file_path::generate_disk(self.root_dir.clone(), extension)
    }

    #[tracing::instrument(level = "debug", skip(self, path), fields(path = ?path.as_ref()))]
    async fn safe_remove_file<P: AsRef<Path>>(&self, path: P) -> Result<(), FileError> {
        tokio::fs::remove_file(&path).await?;
        self.try_remove_parents(path.as_ref()).await;
        Ok(())
    }

    async fn try_remove_parents(&self, mut path: &Path) {
        while let Some(parent) = path.parent() {
            tracing::trace!("try_remove_parents: looping");

            if parent.ends_with(&self.root_dir) {
                return;
            }

            if tokio::fs::remove_dir(parent).await.is_err() {
                return;
            }

            path = parent;
        }
    }

    async fn safe_save_stream<P: AsRef<Path>>(
        &self,
        to: P,
        input: impl Stream<Item = std::io::Result<Bytes>>,
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

        file.write_from_stream(input)
            .with_poll_timer("write-from-stream")
            .await?;

        file.close().await?;

        Ok(())
    }
}

pub(crate) async fn safe_create_parent<P: AsRef<Path>>(path: P) -> Result<(), FileError> {
    if let Some(path) = path.as_ref().parent() {
        tokio::fs::create_dir_all(path).await?;
    }

    Ok(())
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore")
            .field("root_dir", &self.root_dir)
            .finish()
    }
}
