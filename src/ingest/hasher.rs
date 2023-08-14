use sha2::{digest::FixedOutputReset, Digest};
use std::{
    cell::RefCell,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

struct State<D> {
    hasher: D,
    size: u64,
}

pin_project_lite::pin_project! {
    pub(crate) struct Hasher<I, D> {
        #[pin]
        inner: I,

        state: Rc<RefCell<State<D>>>,
    }
}

impl<I, D> Hasher<I, D>
where
    D: Digest + FixedOutputReset + Send + 'static,
{
    pub(super) fn new(reader: I, digest: D) -> Self {
        Hasher {
            inner: reader,
            state: Rc::new(RefCell::new(State {
                hasher: digest,
                size: 0,
            })),
        }
    }

    pub(super) fn state(&self) -> Rc<RefCell<State<D>>> {
        Rc::clone(&self.state)
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
        let state = this.state;

        let before_len = buf.filled().len();
        let poll_res = reader.poll_read(cx, buf);
        let after_len = buf.filled().len();
        if after_len > before_len {
            let mut guard = state.borrow_mut();
            guard.hasher.update(&buf.filled()[before_len..after_len]);
            guard.size += u64::try_from(after_len - before_len).expect("Size is reasonable");
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

            let mut reader = Hasher::new(file1, Sha256::new());

            tokio::io::copy(&mut reader, &mut tokio::io::sink()).await?;

            Ok(reader.state().borrow_mut().hasher.finalize_reset().to_vec()) as std::io::Result<_>
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
