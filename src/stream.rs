use crate::error::Error;
use actix_rt::task::JoinHandle;
use actix_web::web::{Bytes, BytesMut};
use futures_util::Stream;
use std::{
    future::Future,
    pin::Pin,
    process::Stdio,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWriteExt, ReadBuf},
    process::{Child, Command},
    sync::oneshot::{channel, Receiver},
};
use tokio_util::codec::{BytesCodec, FramedRead};
use tracing::instrument;

#[derive(Debug)]
struct StatusError;

pub(crate) struct Process {
    child: Child,
}

pub(crate) struct ProcessRead<I> {
    inner: I,
    err_recv: Receiver<std::io::Error>,
    err_closed: bool,
    handle: JoinHandle<()>,
}

struct BytesFreezer<S>(S);

impl Process {
    fn new(child: Child) -> Self {
        Process { child }
    }

    #[instrument(name = "Spawning command")]
    pub(crate) fn run(command: &str, args: &[&str]) -> std::io::Result<Self> {
        tracing::info!("Spawning");
        Self::spawn(Command::new(command).args(args))
    }

    pub(crate) fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map(Process::new)
    }

    pub(crate) fn bytes_read(mut self, mut input: Bytes) -> Option<impl AsyncRead + Unpin> {
        let mut stdin = self.child.stdin.take()?;
        let stdout = self.child.stdout.take()?;

        let (tx, rx) = channel();

        let mut child = self.child;

        let handle = actix_rt::spawn(async move {
            if let Err(e) = stdin.write_all_buf(&mut input).await {
                let _ = tx.send(e);
                return;
            }
            drop(stdin);

            match child.wait().await {
                Ok(status) => {
                    if !status.success() {
                        let _ =
                            tx.send(std::io::Error::new(std::io::ErrorKind::Other, &StatusError));
                    }
                }
                Err(e) => {
                    let _ = tx.send(e);
                }
            }
        });

        Some(Box::pin(ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
            handle,
        }))
    }

    pub(crate) fn write_read(
        mut self,
        mut input_reader: impl AsyncRead + Unpin + 'static,
    ) -> Option<impl AsyncRead + Unpin> {
        let mut stdin = self.child.stdin.take()?;
        let stdout = self.child.stdout.take()?;

        let (tx, rx) = channel();

        let mut child = self.child;

        let handle = actix_rt::spawn(async move {
            if let Err(e) = tokio::io::copy(&mut input_reader, &mut stdin).await {
                let _ = tx.send(e);
                return;
            }
            drop(stdin);

            match child.wait().await {
                Ok(status) => {
                    if !status.success() {
                        let _ =
                            tx.send(std::io::Error::new(std::io::ErrorKind::Other, &StatusError));
                    }
                }
                Err(e) => {
                    let _ = tx.send(e);
                }
            }
        });

        Some(Box::pin(ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
            handle,
        }))
    }
}

pub(crate) fn bytes_stream(
    input: impl AsyncRead + Unpin,
) -> impl Stream<Item = Result<Bytes, Error>> + Unpin {
    BytesFreezer(FramedRead::new(input, BytesCodec::new()))
}

impl<I> AsyncRead for ProcessRead<I>
where
    I: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.err_closed {
            if let Poll::Ready(res) = Pin::new(&mut self.err_recv).poll(cx) {
                self.err_closed = true;
                if let Ok(err) = res {
                    return Poll::Ready(Err(err));
                }
            }
        }

        if let Poll::Ready(res) = Pin::new(&mut self.inner).poll_read(cx, buf) {
            return Poll::Ready(res);
        }

        Poll::Pending
    }
}

impl<I> Drop for ProcessRead<I> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl<S> Stream for BytesFreezer<S>
where
    S: Stream<Item = std::io::Result<BytesMut>> + Unpin,
{
    type Item = Result<Bytes, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map(|opt| opt.map(|res| res.map(|bytes_mut| bytes_mut.freeze())))
            .map_err(Error::from)
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Command failed with bad status")
    }
}

impl std::error::Error for StatusError {}
