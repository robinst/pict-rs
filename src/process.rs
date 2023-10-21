use actix_web::web::Bytes;
use flume::r#async::RecvFut;
use std::{
    future::Future,
    pin::Pin,
    process::{ExitStatus, Stdio},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWriteExt, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};
use tracing::{Instrument, Span};

use crate::{error_code::ErrorCode, future::WithTimeout};

struct MetricsGuard {
    start: Instant,
    armed: bool,
    command: String,
}

impl MetricsGuard {
    fn guard(command: String) -> Self {
        metrics::increment_counter!("pict-rs.process.start", "command" => command.clone());

        Self {
            start: Instant::now(),
            armed: true,
            command,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for MetricsGuard {
    fn drop(&mut self) {
        metrics::histogram!(
            "pict-rs.process.duration",
            self.start.elapsed().as_secs_f64(),
            "command" => self.command.clone(),
            "completed" => (!self.armed).to_string(),
        );

        metrics::increment_counter!("pict-rs.process.end", "completed" => (!self.armed).to_string() , "command" => self.command.clone());
    }
}

#[derive(Debug)]
struct StatusError(ExitStatus);

pub(crate) struct Process {
    command: String,
    child: Child,
    guard: MetricsGuard,
    timeout: Duration,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process").field("child", &"Child").finish()
    }
}

struct DropHandle {
    inner: JoinHandle<()>,
}

pub(crate) struct ProcessRead<I> {
    inner: I,
    err_recv: RecvFut<'static, std::io::Error>,
    err_closed: bool,
    #[allow(dead_code)]
    handle: DropHandle,
    eof: bool,
    sleep: Pin<Box<tokio::time::Sleep>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    #[error("Required command {0} not found, make sure it exists in pict-rs' $PATH")]
    NotFound(String),

    #[error("Cannot run command {0} due to invalid permissions on binary, make sure the pict-rs user has permission to run it")]
    PermissionDenied(String),

    #[error("Reached process spawn limit")]
    LimitReached,

    #[error("{0} timed out")]
    Timeout(String),

    #[error("{0} Failed with {1}")]
    Status(String, ExitStatus),

    #[error("Unknown process error")]
    Other(#[source] std::io::Error),
}

impl ProcessError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::COMMAND_NOT_FOUND,
            Self::PermissionDenied(_) => ErrorCode::COMMAND_PERMISSION_DENIED,
            Self::LimitReached | Self::Other(_) => ErrorCode::COMMAND_ERROR,
            Self::Timeout(_) => ErrorCode::COMMAND_TIMEOUT,
            Self::Status(_, _) => ErrorCode::COMMAND_FAILURE,
        }
    }
}

impl Process {
    pub(crate) fn run(command: &str, args: &[&str], timeout: u64) -> Result<Self, ProcessError> {
        let res = tracing::trace_span!(parent: None, "Create command", %command)
            .in_scope(|| Self::spawn(command, Command::new(command).args(args), timeout));

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

    fn spawn(command: &str, cmd: &mut Command, timeout: u64) -> std::io::Result<Self> {
        tracing::trace_span!(parent: None, "Spawn command", %command).in_scope(|| {
            let guard = MetricsGuard::guard(command.into());

            let cmd = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true);

            cmd.spawn().map(|child| Process {
                child,
                command: String::from(command),
                guard,
                timeout: Duration::from_secs(timeout),
            })
        })
    }

    #[tracing::instrument(skip(self))]
    pub(crate) async fn wait(self) -> Result<(), ProcessError> {
        let Process {
            command,
            mut child,
            guard,
            timeout,
        } = self;

        let res = child.wait().with_timeout(timeout).await;

        match res {
            Ok(Ok(status)) if status.success() => {
                guard.disarm();

                Ok(())
            }
            Ok(Ok(status)) => Err(ProcessError::Status(command, status)),
            Ok(Err(e)) => Err(ProcessError::Other(e)),
            Err(_) => {
                child.kill().await.map_err(ProcessError::Other)?;

                Err(ProcessError::Timeout(command))
            }
        }
    }

    pub(crate) fn bytes_read(self, input: Bytes) -> ProcessRead<ChildStdout> {
        self.spawn_fn(move |mut stdin| {
            let mut input = input;
            async move { stdin.write_all_buf(&mut input).await }
        })
    }

    pub(crate) fn read(self) -> ProcessRead<ChildStdout> {
        self.spawn_fn(|_| async { Ok(()) })
    }

    #[allow(unknown_lints)]
    #[allow(clippy::let_with_type_underscore)]
    #[tracing::instrument(level = "trace", skip_all)]
    fn spawn_fn<F, Fut>(self, f: F) -> ProcessRead<ChildStdout>
    where
        F: FnOnce(ChildStdin) -> Fut + 'static,
        Fut: Future<Output = std::io::Result<()>>,
    {
        let Process {
            command,
            mut child,
            guard,
            timeout,
        } = self;

        let stdin = child.stdin.take().expect("stdin exists");
        let stdout = child.stdout.take().expect("stdout exists");

        let (tx, rx) = crate::sync::channel::<std::io::Error>(1);
        let rx = rx.into_recv_async();

        let span = tracing::info_span!(parent: None, "Background process task", %command);
        span.follows_from(Span::current());

        let handle = crate::sync::spawn(
            "await-process",
            async move {
                let child_fut = async {
                    (f)(stdin).await?;

                    child.wait().await
                };

                let error = match child_fut.with_timeout(timeout).await {
                    Ok(Ok(status)) if status.success() => {
                        guard.disarm();
                        return;
                    }
                    Ok(Ok(status)) => {
                        std::io::Error::new(std::io::ErrorKind::Other, StatusError(status))
                    }
                    Ok(Err(e)) => e,
                    Err(_) => std::io::ErrorKind::TimedOut.into(),
                };

                let _ = tx.send(error);
                let _ = child.kill().await;
            }
            .instrument(span),
        );

        let sleep = tokio::time::sleep(timeout);

        ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
            handle: DropHandle { inner: handle },
            eof: false,
            sleep: Box::pin(sleep),
        }
    }
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

                if self.eof {
                    return Poll::Ready(Ok(()));
                }
            }

            if let Poll::Ready(()) = self.sleep.as_mut().poll(cx) {
                self.err_closed = true;

                return Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()));
            }
        }

        if !self.eof {
            let before_size = buf.filled().len();

            return match Pin::new(&mut self.inner).poll_read(cx, buf) {
                Poll::Ready(Ok(())) => {
                    if buf.filled().len() == before_size {
                        self.eof = true;

                        if !self.err_closed {
                            // reached end of stream & haven't received process signal
                            return Poll::Pending;
                        }
                    }

                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(e)) => {
                    self.eof = true;

                    Poll::Ready(Err(e))
                }
                Poll::Pending => Poll::Pending,
            };
        }

        if self.err_closed && self.eof {
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
