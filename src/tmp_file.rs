use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};
use tokio::io::AsyncRead;
use uuid::Uuid;

pub(crate) type ArcTmpDir = Arc<TmpDir>;

#[derive(Debug)]
pub(crate) struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    pub(crate) async fn init() -> std::io::Result<Arc<Self>> {
        let path = std::env::temp_dir().join(Uuid::new_v4().to_string());
        tokio::fs::create_dir(&path).await?;
        Ok(Arc::new(TmpDir { path }))
    }

    pub(crate) fn tmp_file(&self, ext: Option<&str>) -> PathBuf {
        if let Some(ext) = ext {
            self.path.join(format!("{}{}", Uuid::new_v4(), ext))
        } else {
            self.path.join(Uuid::new_v4().to_string())
        }
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).expect("Removed directory");
    }
}

static TMP_DIR: OnceLock<PathBuf> = OnceLock::new();

fn tmp_dir() -> &'static PathBuf {
    TMP_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        dir
    })
}

struct TmpFile(PathBuf);

impl Drop for TmpFile {
    fn drop(&mut self) {
        crate::sync::spawn(tokio::fs::remove_file(self.0.clone()));
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct TmpFileCleanup<R> {
        #[pin]
        inner: R,

        file: TmpFile,
    }
}

pub(crate) fn tmp_file(ext: Option<&str>) -> PathBuf {
    if let Some(ext) = ext {
        tmp_dir().join(format!("{}{}", Uuid::new_v4(), ext))
    } else {
        tmp_dir().join(Uuid::new_v4().to_string())
    }
}

pub(crate) async fn remove_tmp_dir() -> std::io::Result<()> {
    tokio::fs::remove_dir_all(tmp_dir()).await
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
