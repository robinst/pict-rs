use std::{path::PathBuf, sync::Arc};
use tokio::io::AsyncRead;
use uuid::Uuid;

pub(crate) type ArcTmpDir = Arc<TmpDir>;

#[derive(Debug)]
pub(crate) struct TmpDir {
    path: Option<PathBuf>,
}

impl TmpDir {
    pub(crate) async fn init() -> std::io::Result<Arc<Self>> {
        let path = std::env::temp_dir().join(Uuid::new_v4().to_string());
        tokio::fs::create_dir(&path).await?;
        Ok(Arc::new(TmpDir { path: Some(path) }))
    }

    pub(crate) fn tmp_file(&self, ext: Option<&str>) -> PathBuf {
        if let Some(ext) = ext {
            self.path
                .as_ref()
                .expect("tmp path exists")
                .join(format!("{}{}", Uuid::new_v4(), ext))
        } else {
            self.path
                .as_ref()
                .expect("tmp path exists")
                .join(Uuid::new_v4().to_string())
        }
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

struct TmpFile(PathBuf);

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

pub(crate) fn cleanup_tmpfile<R: AsyncRead>(reader: R, file: PathBuf) -> TmpFileCleanup<R> {
    TmpFileCleanup {
        inner: reader,
        file: TmpFile(file),
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
