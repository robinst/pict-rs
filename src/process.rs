use crate::store::Store;
use actix_rt::task::JoinHandle;
use actix_web::web::Bytes;
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
use tracing::{Instrument, Span};

#[derive(Debug)]
struct StatusError;

pub(crate) struct Process {
    child: Child,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process").field("child", &"Child").finish()
    }
}

struct DropHandle {
    inner: JoinHandle<()>,
}

pin_project_lite::pin_project! {
    struct ProcessRead<I> {
        #[pin]
        inner: I,
        err_recv: Receiver<std::io::Error>,
        err_closed: bool,
        handle: DropHandle,
    }
}

impl Process {
    #[tracing::instrument]
    pub(crate) fn run(command: &str, args: &[&str]) -> std::io::Result<Self> {
        tracing::trace_span!(parent: None, "Create command")
            .in_scope(|| Self::spawn(Command::new(command).args(args)))
    }

    #[tracing::instrument]
    pub(crate) fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        tracing::trace_span!(parent: None, "Spawn command").in_scope(|| {
            let cmd = cmd.stdin(Stdio::piped()).stdout(Stdio::piped());

            cmd.spawn().map(|child| Process { child })
        })
    }

    #[tracing::instrument]
    pub(crate) async fn wait(mut self) -> std::io::Result<()> {
        let status = self.child.wait().await?;
        if !status.success() {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, &StatusError));
        }
        Ok(())
    }

    #[tracing::instrument(skip(input))]
    pub(crate) fn bytes_read(mut self, mut input: Bytes) -> impl AsyncRead + Unpin {
        let mut stdin = self.child.stdin.take().expect("stdin exists");
        let stdout = self.child.stdout.take().expect("stdout exists");

        let (tx, rx) = tracing::trace_span!(parent: None, "Create channel")
            .in_scope(channel::<std::io::Error>);

        let span = tracing::info_span!(parent: None, "Background process task from bytes");
        span.follows_from(Span::current());

        let mut child = self.child;
        let handle = tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
            actix_rt::spawn(
                async move {
                    if let Err(e) = stdin.write_all_buf(&mut input).await {
                        let _ = tx.send(e);
                        return;
                    }
                    drop(stdin);

                    match child.wait().await {
                        Ok(status) => {
                            if !status.success() {
                                let _ = tx.send(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    &StatusError,
                                ));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(e);
                        }
                    }
                }
                .instrument(span),
            )
        });

        ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
            handle: DropHandle { inner: handle },
        }
    }

    #[tracing::instrument]
    pub(crate) fn read(mut self) -> impl AsyncRead + Unpin {
        let stdout = self.child.stdout.take().expect("stdout exists");

        let (tx, rx) = tracing::trace_span!(parent: None, "Create channel").in_scope(channel);

        let span = tracing::info_span!(parent: None, "Background process task");
        span.follows_from(Span::current());

        let mut child = self.child;
        let handle = tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
            actix_rt::spawn(
                async move {
                    match child.wait().await {
                        Ok(status) => {
                            if !status.success() {
                                let _ = tx.send(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    &StatusError,
                                ));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(e);
                        }
                    }
                }
                .instrument(span),
            )
        });

        ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
            handle: DropHandle { inner: handle },
        }
    }

    pub(crate) fn pipe_async_read<A: AsyncRead + Unpin + 'static>(
        mut self,
        mut async_read: A,
    ) -> impl AsyncRead + Unpin {
        let mut stdin = self.child.stdin.take().expect("stdin exists");
        let stdout = self.child.stdout.take().expect("stdout exists");

        let (tx, rx) = tracing::trace_span!(parent: None, "Create channel")
            .in_scope(channel::<std::io::Error>);

        let span = tracing::info_span!(parent: None, "Background process task from bytes");
        span.follows_from(Span::current());

        let mut child = self.child;
        let handle = tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
            actix_rt::spawn(
                async move {
                    if let Err(e) = tokio::io::copy(&mut async_read, &mut stdin).await {
                        let _ = tx.send(e);
                        return;
                    }
                    drop(stdin);

                    match child.wait().await {
                        Ok(status) => {
                            if !status.success() {
                                let _ = tx.send(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    &StatusError,
                                ));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(e);
                        }
                    }
                }
                .instrument(span),
            )
        });

        ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
            handle: DropHandle { inner: handle },
        }
    }

    #[tracing::instrument]
    pub(crate) fn store_read<S: Store + 'static>(
        mut self,
        store: S,
        identifier: S::Identifier,
    ) -> impl AsyncRead + Unpin {
        let mut stdin = self.child.stdin.take().expect("stdin exists");
        let stdout = self.child.stdout.take().expect("stdout exists");

        let (tx, rx) = tracing::trace_span!(parent: None, "Create channel").in_scope(channel);

        let span = tracing::info_span!(
            parent: None,
            "Background processs task from store",
            ?store,
            ?identifier
        );
        span.follows_from(Span::current());

        let mut child = self.child;
        let handle = tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
            actix_rt::spawn(
                async move {
                    if let Err(e) = store.read_into(&identifier, &mut stdin).await {
                        let _ = tx.send(e);
                        return;
                    }
                    drop(stdin);

                    match child.wait().await {
                        Ok(status) => {
                            if !status.success() {
                                let _ = tx.send(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    &StatusError,
                                ));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(e);
                        }
                    }
                }
                .instrument(span),
            )
        });

        ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
            handle: DropHandle { inner: handle },
        }
    }
}

impl<I> AsyncRead for ProcessRead<I>
where
    I: AsyncRead,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.as_mut().project();

        let err_recv = this.err_recv;
        let err_closed = this.err_closed;
        let inner = this.inner;

        if !*err_closed {
            if let Poll::Ready(res) = Pin::new(err_recv).poll(cx) {
                *err_closed = true;
                if let Ok(err) = res {
                    return Poll::Ready(Err(err));
                }
            }
        }

        inner.poll_read(cx, buf)
    }
}

impl Drop for DropHandle {
    fn drop(&mut self) {
        self.inner.abort();
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Command failed with bad status")
    }
}

impl std::error::Error for StatusError {}
