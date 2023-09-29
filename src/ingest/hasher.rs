use sha2::{digest::FixedOutputReset, Digest, Sha256};
use std::{
    cell::RefCell,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

pub(super) struct State {
    hasher: Sha256,
    size: u64,
}

pin_project_lite::pin_project! {
    pub(crate) struct Hasher<I> {
        #[pin]
        inner: I,

        state: Rc<RefCell<State>>,
    }
}

impl<I> Hasher<I> {
    pub(super) fn new(reader: I) -> Self {
        Hasher {
            inner: reader,
            state: Rc::new(RefCell::new(State {
                hasher: Sha256::new(),
                size: 0,
            })),
        }
    }

    pub(super) fn state(&self) -> Rc<RefCell<State>> {
        Rc::clone(&self.state)
    }
}

impl State {
    pub(super) fn finalize_reset(&mut self) -> ([u8; 32], u64) {
        let arr = self.hasher.finalize_fixed_reset().into();

        (arr, self.size)
    }
}

impl<I> AsyncRead for Hasher<I>
where
    I: AsyncRead,
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

    macro_rules! test_async {
        ($fut:expr) => {
            actix_web::rt::System::new()
                .block_on(async move { crate::sync::spawn($fut).await.unwrap() })
        };
    }

    #[test]
    fn hasher_works() {
        let (hash, size) = test_async!(async move {
            let file1 = tokio::fs::File::open("./client-examples/earth.gif").await?;

            let mut reader = Hasher::new(file1);

            tokio::io::copy(&mut reader, &mut tokio::io::sink()).await?;

            Ok(reader.state().borrow_mut().finalize_reset()) as std::io::Result<_>
        })
        .unwrap();

        let mut file = std::fs::File::open("./client-examples/earth.gif").unwrap();
        let mut vec = Vec::new();
        file.read_to_end(&mut vec).unwrap();
        let correct_size = vec.len() as u64;
        let mut hasher = Sha256::new();
        hasher.update(vec);
        let correct_hash: [u8; 32] = hasher.finalize_reset().into();

        assert_eq!(hash, correct_hash);
        assert_eq!(size, correct_size);
    }
}
