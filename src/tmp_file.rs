use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

use uuid::Uuid;

use crate::process::Extras;

pub(crate) type ArcTmpDir = Arc<TmpDir>;

#[derive(Debug)]
pub(crate) struct TmpDir {
    path: Option<PathBuf>,
}

impl TmpDir {
    pub(crate) async fn init<P: AsRef<Path>>(path: P, cleanup: bool) -> std::io::Result<Arc<Self>> {
        let base_path = path.as_ref().join("pict-rs");

        if cleanup && tokio::fs::metadata(&base_path).await.is_ok() {
            tokio::fs::remove_dir_all(&base_path).await?;
        }

        let path = base_path.join(Uuid::now_v7().to_string());

        tokio::fs::create_dir_all(&path).await?;

        Ok(Arc::new(TmpDir { path: Some(path) }))
    }

    fn build_tmp_file(&self, ext: Option<&str>) -> PathBuf {
        if let Some(ext) = ext {
            self.path
                .as_ref()
                .expect("tmp path exists")
                .join(format!("{}{}", Uuid::now_v7(), ext))
        } else {
            self.path
                .as_ref()
                .expect("tmp path exists")
                .join(Uuid::now_v7().to_string())
        }
    }

    pub(crate) fn tmp_file(&self, ext: Option<&str>) -> TmpFile {
        TmpFile(Some(self.build_tmp_file(ext)))
    }

    pub(crate) async fn tmp_folder(&self) -> std::io::Result<TmpFolder> {
        let path = self.build_tmp_file(None);
        tokio::fs::create_dir(&path).await?;
        Ok(TmpFolder(Some(path)))
    }

    pub(crate) async fn cleanup(self: Arc<Self>) -> std::io::Result<()> {
        if let Some(mut path) = Arc::into_inner(self).and_then(|mut this| this.path.take()) {
            tokio::fs::remove_dir_all(&path).await?;

            if path.pop() {
                // attempt to remove parent directory if it is empty
                let _ = tokio::fs::remove_dir(path).await;
            }
        }

        Ok(())
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        if let Some(mut path) = self.path.take() {
            tracing::warn!("TmpDir - Blocking remove of {path:?}");
            std::fs::remove_dir_all(&path).expect("Removed directory");
            if path.pop() {
                // attempt to remove parent directory if it is empty
                let _ = std::fs::remove_dir(path);
            }
        }
    }
}

#[must_use]
pub(crate) struct TmpFolder(Option<PathBuf>);

impl TmpFolder {
    pub(crate) async fn cleanup(mut self) -> std::io::Result<()> {
        self.consume().await
    }
}

#[async_trait::async_trait(?Send)]
impl Extras for TmpFolder {
    async fn consume(&mut self) -> std::io::Result<()> {
        tokio::fs::remove_dir_all(&self).await?;
        self.0.take();
        Ok(())
    }
}

impl AsRef<Path> for TmpFolder {
    fn as_ref(&self) -> &Path {
        self.0.as_deref().unwrap()
    }
}

impl Deref for TmpFolder {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().unwrap()
    }
}

impl Drop for TmpFolder {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            tracing::warn!("TmpFolder - Blocking remove of directory {path:?}");
            if let Err(e) = std::fs::remove_dir_all(path) {
                tracing::error!("Failed removing directory {e}");
            }
        }
    }
}

#[must_use]
pub(crate) struct TmpFile(Option<PathBuf>);

impl TmpFile {
    pub(crate) async fn cleanup(mut self) -> std::io::Result<()> {
        self.consume().await
    }
}

#[async_trait::async_trait(?Send)]
impl Extras for TmpFile {
    async fn consume(&mut self) -> std::io::Result<()> {
        tokio::fs::remove_file(&self).await?;
        self.0.take();
        Ok(())
    }
}

impl AsRef<Path> for TmpFile {
    fn as_ref(&self) -> &Path {
        self.0.as_deref().unwrap()
    }
}

impl Deref for TmpFile {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().unwrap()
    }
}

impl Drop for TmpFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            tracing::warn!("TmpFile - Blocking remove of file {path:?}");
            if let Err(e) = std::fs::remove_file(path) {
                tracing::error!("Failed removing file {e}");
            }
        }
    }
}
