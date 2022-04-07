use once_cell::sync::Lazy;
use std::path::PathBuf;
use tokio::io::AsyncRead;
use uuid::Uuid;

static TMP_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let dir = std::env::temp_dir().join(Uuid::new_v4().to_string());
    std::fs::create_dir(&dir).unwrap();
    dir
});

struct TmpFile(PathBuf);

impl Drop for TmpFile {
    fn drop(&mut self) {
        tracing::trace_span!(parent: None, "Spawn task")
            .in_scope(|| actix_rt::spawn(tokio::fs::remove_file(self.0.clone())));
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
        TMP_DIR.join(format!("{}{}", Uuid::new_v4(), ext))
    } else {
        TMP_DIR.join(Uuid::new_v4().to_string())
    }
}

pub(crate) async fn remove_tmp_dir() -> std::io::Result<()> {
    tokio::fs::remove_dir_all(&*TMP_DIR).await
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
