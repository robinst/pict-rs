use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncRead;
use uuid::Uuid;

pub(crate) type ArcTmpDir = Arc<TmpDir>;

#[derive(Debug)]
pub(crate) struct TmpDir {
    path: Option<PathBuf>,
}

impl TmpDir {
    pub(crate) async fn init<P: AsRef<Path>>(path: P) -> std::io::Result<Arc<Self>> {
        let path = path.as_ref().join(Uuid::new_v4().to_string());
        tokio::fs::create_dir(&path).await?;
        Ok(Arc::new(TmpDir { path: Some(path) }))
    }

    fn build_tmp_file(&self, ext: Option<&str>) -> Arc<Path> {
        if let Some(ext) = ext {
            Arc::from(self.path.as_ref().expect("tmp path exists").join(format!(
                "{}{}",
                Uuid::new_v4(),
                ext
            )))
        } else {
            Arc::from(
                self.path
                    .as_ref()
                    .expect("tmp path exists")
                    .join(Uuid::new_v4().to_string()),
            )
        }
    }

    pub(crate) fn tmp_file(&self, ext: Option<&str>) -> TmpFile {
        TmpFile(self.build_tmp_file(ext))
    }

    pub(crate) async fn tmp_folder(&self) -> std::io::Result<TmpFolder> {
        let path = self.build_tmp_file(None);
        tokio::fs::create_dir(&path).await?;
        Ok(TmpFolder(path))
    }

    pub(crate) async fn cleanup(self: Arc<Self>) -> std::io::Result<()> {
        if let Some(path) = Arc::into_inner(self).and_then(|mut this| this.path.take()) {
            tokio::fs::remove_dir_all(path).await?;
        }

        Ok(())
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        if let Some(path) = self.path.as_ref() {
            std::fs::remove_dir_all(path).expect("Removed directory");
        }
    }
}

#[must_use]
pub(crate) struct TmpFolder(Arc<Path>);

impl TmpFolder {
    pub(crate) fn reader<R: AsyncRead>(self, reader: R) -> TmpFolderCleanup<R> {
        TmpFolderCleanup {
            inner: reader,
            folder: self,
        }
    }
}

impl AsRef<Path> for TmpFolder {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Deref for TmpFolder {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TmpFolder {
    fn drop(&mut self) {
        crate::sync::spawn(
            "remove-tmpfolder",
            tokio::fs::remove_dir_all(self.0.clone()),
        );
    }
}

#[must_use]
pub(crate) struct TmpFile(Arc<Path>);

impl TmpFile {
    pub(crate) fn reader<R: AsyncRead>(self, reader: R) -> TmpFileCleanup<R> {
        TmpFileCleanup {
            inner: reader,
            file: self,
        }
    }
}

impl AsRef<Path> for TmpFile {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Deref for TmpFile {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for TmpFile {
    fn drop(&mut self) {
        crate::sync::spawn("remove-tmpfile", tokio::fs::remove_file(self.0.clone()));
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct TmpFileCleanup<R> {
        #[pin]
        inner: R,

        file: TmpFile,
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct TmpFolderCleanup<R> {
        #[pin]
        inner: R,

        folder: TmpFolder,
    }
}

impl<R: AsyncRead> AsyncRead for TmpFileCleanup<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.as_mut().project();

        this.inner.poll_read(cx, buf)
    }
}

impl<R: AsyncRead> AsyncRead for TmpFolderCleanup<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.as_mut().project();

        this.inner.poll_read(cx, buf)
    }
}
