use crate::store::Store;
use actix_rt::task::JoinHandle;
use actix_web::web::Bytes;
use std::{
    future::Future,
    pin::Pin,
    process::{ExitStatus, Stdio},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWriteExt, ReadBuf},
    process::{Child, ChildStdin, Command},
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

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    #[error("Required commend {0} not found")]
    NotFound(String),

    #[error("Reached process spawn limit")]
    LimitReached,

    #[error("Failed with status {0}")]
    Status(ExitStatus),

    #[error("Unknown process error")]
    Other(#[source] std::io::Error),
}

impl Process {
    pub(crate) fn run(command: &str, args: &[&str]) -> Result<Self, ProcessError> {
        let res = tracing::trace_span!(parent: None, "Create command")
            .in_scope(|| Self::spawn(Command::new(command).args(args)));

        match res {
            Ok(this) => Ok(this),
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => Err(ProcessError::NotFound(command.to_string())),
                std::io::ErrorKind::WouldBlock => Err(ProcessError::LimitReached),
                _ => Err(ProcessError::Other(e)),
            },
        }
    }

    fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        tracing::trace_span!(parent: None, "Spawn command").in_scope(|| {
            let cmd = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true);

            cmd.spawn().map(|child| Process { child })
        })
    }

    #[tracing::instrument(skip(self))]
    pub(crate) async fn wait(mut self) -> Result<(), ProcessError> {
        let res = self.child.wait().await;

        match res {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(ProcessError::Status(status)),
            Err(e) => Err(ProcessError::Other(e)),
        }
    }

    pub(crate) fn bytes_read(self, input: Bytes) -> impl AsyncRead + Unpin {
        self.spawn_fn(move |mut stdin| {
            let mut input = input;
            async move { stdin.write_all_buf(&mut input).await }
        })
    }

    pub(crate) fn read(self) -> impl AsyncRead + Unpin {
        self.spawn_fn(|_| async { Ok(()) })
    }

    pub(crate) fn pipe_async_read<A: AsyncRead + Unpin + 'static>(
        self,
        mut async_read: A,
    ) -> impl AsyncRead + Unpin {
        self.spawn_fn(move |mut stdin| async move {
            tokio::io::copy(&mut async_read, &mut stdin)
                .await
                .map(|_| ())
        })
    }

    pub(crate) fn store_read<S: Store + 'static>(
        self,
        store: S,
        identifier: S::Identifier,
    ) -> impl AsyncRead + Unpin {
        self.spawn_fn(move |mut stdin| {
            let store = store;
            let identifier = identifier;

            async move { store.read_into(&identifier, &mut stdin).await }
        })
    }

    #[allow(unknown_lints)]
    #[allow(clippy::let_with_type_underscore)]
    #[tracing::instrument(level = "trace", skip_all)]
    fn spawn_fn<F, Fut>(mut self, f: F) -> impl AsyncRead + Unpin
    where
        F: FnOnce(ChildStdin) -> Fut + 'static,
        Fut: Future<Output = std::io::Result<()>>,
    {
        let stdin = self.child.stdin.take().expect("stdin exists");
        let stdout = self.child.stdout.take().expect("stdout exists");

        let (tx, rx) = tracing::trace_span!(parent: None, "Create channel")
            .in_scope(channel::<std::io::Error>);

        let span = tracing::info_span!(parent: None, "Background process task");
        span.follows_from(Span::current());

        let mut child = self.child;
        let handle = tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
            actix_rt::spawn(
                async move {
                    if let Err(e) = (f)(stdin).await {
                        let _ = tx.send(e);
                        return;
                    }

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
