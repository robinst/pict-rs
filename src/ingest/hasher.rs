use crate::error::Error;
use actix_web::web;
use sha2::{digest::FixedOutputReset, Digest};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

pin_project_lite::pin_project! {
    pub(crate) struct Hasher<I, D> {
        #[pin]
        inner: I,

        hasher: D,
    }
}

impl<I, D> Hasher<I, D>
where
    D: Digest + FixedOutputReset + Send + 'static,
{
    pub(super) fn new(reader: I, digest: D) -> Self {
        Hasher {
            inner: reader,
            hasher: digest,
        }
    }

    pub(super) async fn finalize_reset(self) -> Result<Vec<u8>, Error> {
        let mut hasher = self.hasher;
        let hash = web::block(move || hasher.finalize_reset().to_vec()).await?;
        Ok(hash)
    }
}

impl<I, D> AsyncRead for Hasher<I, D>
where
    I: AsyncRead,
    D: Digest,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.as_mut().project();

        let reader = this.inner;
        let hasher = this.hasher;

        let before_len = buf.filled().len();
        let poll_res = reader.poll_read(cx, buf);
        let after_len = buf.filled().len();
        if after_len > before_len {
            hasher.update(&buf.filled()[before_len..after_len]);
        }
        poll_res
    }
}

#[cfg(test)]
mod test {
    use super::Hasher;
    use sha2::{Digest, Sha256};
    use std::io::Read;

    macro_rules! test_on_arbiter {
        ($fut:expr) => {
            actix_rt::System::new().block_on(async move {
                let arbiter = actix_rt::Arbiter::new();

                let (tx, rx) = tracing::trace_span!(parent: None, "Create channel")
                    .in_scope(|| tokio::sync::oneshot::channel());

                arbiter.spawn(async move {
                    let handle = actix_rt::spawn($fut);

                    let _ = tx.send(handle.await.unwrap());
                });

                rx.await.unwrap()
            })
        };
    }

    #[test]
    fn hasher_works() {
        let hash = test_on_arbiter!(async move {
            let file1 = tokio::fs::File::open("./client-examples/earth.gif").await?;

            let mut hasher = Hasher::new(file1, Sha256::new());

            tokio::io::copy(&mut hasher, &mut tokio::io::sink()).await?;

            hasher.finalize_reset().await
        })
        .unwrap();

        let mut file = std::fs::File::open("./client-examples/earth.gif").unwrap();
        let mut vec = Vec::new();
        file.read_to_end(&mut vec).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(vec);
        let correct_hash = hasher.finalize_reset().to_vec();

        assert_eq!(hash, correct_hash);
    }
}
