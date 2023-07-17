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
struct StatusError(ExitStatus);

pub(crate) struct Process {
    command: String,
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
        eof: bool,
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    #[error("Required command {0} not found, make sure it exists in pict-rs' $PATH")]
    NotFound(String),

    #[error("Cannot run command {0} due to invalid permissions on binary, make sure the pict-rs user has permission to run it")]
    PermissionDenied(String),

    #[error("Reached process spawn limit")]
    LimitReached,

    #[error("Failed with status {0}")]
    Status(ExitStatus),

    #[error("Unknown process error")]
    Other(#[source] std::io::Error),
}

impl Process {
    pub(crate) fn run(command: &str, args: &[&str]) -> Result<Self, ProcessError> {
        let res = tracing::trace_span!(parent: None, "Create command", %command)
            .in_scope(|| Self::spawn(command, Command::new(command).args(args)));

        match res {
            Ok(this) => Ok(this),
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => Err(ProcessError::NotFound(command.to_string())),
                std::io::ErrorKind::PermissionDenied => {
                    Err(ProcessError::PermissionDenied(command.to_string()))
                }
                std::io::ErrorKind::WouldBlock => Err(ProcessError::LimitReached),
                _ => Err(ProcessError::Other(e)),
            },
        }
    }

    fn spawn(command: &str, cmd: &mut Command) -> std::io::Result<Self> {
        tracing::trace_span!(parent: None, "Spawn command", %command).in_scope(|| {
            let cmd = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true);

            cmd.spawn().map(|child| Process {
                child,
                command: String::from(command),
            })
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

        let (tx, rx) = tracing::trace_span!(parent: None, "Create channel", %self.command)
            .in_scope(channel::<std::io::Error>);

        let span = tracing::info_span!(parent: None, "Background process task", %self.command);
        span.follows_from(Span::current());

        let mut child = self.child;
        let command = self.command;
        let handle = tracing::trace_span!(parent: None, "Spawn task", %command).in_scope(|| {
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
                                    StatusError(status),
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
            eof: false,
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
        let eof = this.eof;
        let inner = this.inner;

        if !*err_closed {
            if let Poll::Ready(res) = Pin::new(err_recv).poll(cx) {
                *err_closed = true;

                if let Ok(err) = res {
                    return Poll::Ready(Err(err));
                }

                if *eof {
                    return Poll::Ready(Ok(()));
                }
            }
        }

        if !*eof {
            let before_size = buf.filled().len();

            return match inner.poll_read(cx, buf) {
                Poll::Ready(Ok(())) => {
                    if buf.filled().len() == before_size {
                        *eof = true;

                        if !*err_closed {
                            // reached end of stream & haven't received process signal
                            return Poll::Pending;
                        }
                    }

                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(e)) => {
                    *eof = true;

                    Poll::Ready(Err(e))
                }
                Poll::Pending => Poll::Pending,
            };
        }

        if *err_closed && *eof {
            return Poll::Ready(Ok(()));
        }

        Poll::Pending
    }
}

impl Drop for DropHandle {
    fn drop(&mut self) {
        self.inner.abort();
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Command failed with bad status: {}", self.0)
    }
}

impl std::error::Error for StatusError {}
