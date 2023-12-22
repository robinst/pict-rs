use actix_web::web::Bytes;
use std::{
    ffi::OsStr,
    future::Future,
    pin::Pin,
    process::{ExitStatus, Stdio},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWriteExt, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use tracing::Span;
use uuid::Uuid;

use crate::{
    error_code::ErrorCode,
    future::{LocalBoxFuture, WithTimeout},
};

struct MetricsGuard {
    start: Instant,
    armed: bool,
    command: Arc<str>,
}

impl MetricsGuard {
    fn guard(command: Arc<str>) -> Self {
        metrics::increment_counter!("pict-rs.process.start", "command" => command.to_string());

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
            "command" => self.command.to_string(),
            "completed" => (!self.armed).to_string(),
        );

        metrics::increment_counter!("pict-rs.process.end", "completed" => (!self.armed).to_string() , "command" => self.command.to_string());
    }
}

#[derive(Debug)]
struct StatusError(ExitStatus);

pub(crate) struct Process {
    command: Arc<str>,
    child: Child,
    guard: MetricsGuard,
    timeout: Duration,
    id: Uuid,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process").field("child", &"Child").finish()
    }
}

struct ProcessReadState {
    flags: AtomicU8,
    parent: Mutex<Option<Waker>>,
}

struct ProcessReadWaker {
    state: Arc<ProcessReadState>,
    flag: u8,
}

pub(crate) struct ProcessRead {
    stdout: ChildStdout,
    handle: LocalBoxFuture<'static, std::io::Result<()>>,
    closed: bool,
    state: Arc<ProcessReadState>,
    span: Option<Span>,
    command: Arc<str>,
    id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessError {
    #[error("Required command {0} not found, make sure it exists in pict-rs' $PATH")]
    NotFound(Arc<str>),

    #[error("Cannot run command {0} due to invalid permissions on binary, make sure the pict-rs user has permission to run it")]
    PermissionDenied(Arc<str>),

    #[error("Reached process spawn limit")]
    LimitReached,

    #[error("{0} timed out")]
    Timeout(Arc<str>),

    #[error("{0} Failed with {1}")]
    Status(Arc<str>, ExitStatus),

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
    pub(crate) fn run<T>(
        command: &str,
        args: &[T],
        envs: &[(&str, &OsStr)],
        timeout: u64,
    ) -> Result<Self, ProcessError>
    where
        T: AsRef<OsStr>,
    {
        let command: Arc<str> = Arc::from(String::from(command));

        let res = tracing::trace_span!(parent: None, "Create command", %command).in_scope(|| {
            Self::spawn(
                command.clone(),
                Command::new(command.as_ref())
                    .args(args)
                    .envs(envs.iter().copied()),
                timeout,
            )
        });

        match res {
            Ok(this) => Ok(this),
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => Err(ProcessError::NotFound(command)),
                std::io::ErrorKind::PermissionDenied => {
                    Err(ProcessError::PermissionDenied(command))
                }
                std::io::ErrorKind::WouldBlock => Err(ProcessError::LimitReached),
                _ => Err(ProcessError::Other(e)),
            },
        }
    }

    fn spawn(command: Arc<str>, cmd: &mut Command, timeout: u64) -> std::io::Result<Self> {
        tracing::trace_span!(parent: None, "Spawn command", %command).in_scope(|| {
            let guard = MetricsGuard::guard(command.clone());

            let cmd = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true);

            cmd.spawn().map(|child| Process {
                child,
                command,
                guard,
                timeout: Duration::from_secs(timeout),
                id: Uuid::now_v7(),
            })
        })
    }

    #[tracing::instrument(skip(self), fields(command = %self.command, id = %self.id))]
    pub(crate) async fn wait(self) -> Result<(), ProcessError> {
        let Process {
            command,
            mut child,
            guard,
            timeout,
            id: _,
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

    pub(crate) fn bytes_read(self, input: Bytes) -> ProcessRead {
        self.spawn_fn(move |mut stdin| {
            let mut input = input;
            async move { stdin.write_all_buf(&mut input).await }
        })
    }

    pub(crate) fn read(self) -> ProcessRead {
        self.spawn_fn(|_| async { Ok(()) })
    }

    #[allow(unknown_lints)]
    #[allow(clippy::let_with_type_underscore)]
    #[tracing::instrument(level = "trace", skip_all)]
    fn spawn_fn<F, Fut>(self, f: F) -> ProcessRead
    where
        F: FnOnce(ChildStdin) -> Fut + 'static,
        Fut: Future<Output = std::io::Result<()>>,
    {
        let Process {
            command,
            mut child,
            guard,
            timeout,
            id,
        } = self;

        let stdin = child.stdin.take().expect("stdin exists");
        let stdout = child.stdout.take().expect("stdout exists");

        let handle = Box::pin(async move {
            let child_fut = async {
                (f)(stdin).await?;

                child.wait().await
            };

            let error = match child_fut.with_timeout(timeout).await {
                Ok(Ok(status)) if status.success() => {
                    guard.disarm();
                    return Ok(());
                }
                Ok(Ok(status)) => {
                    std::io::Error::new(std::io::ErrorKind::Other, StatusError(status))
                }
                Ok(Err(e)) => e,
                Err(_) => std::io::ErrorKind::TimedOut.into(),
            };

            child.kill().await?;

            Err(error)
        });

        ProcessRead {
            stdout,
            handle,
            closed: false,
            state: ProcessReadState::new_woken(),
            span: None,
            command,
            id,
        }
    }
}

impl ProcessReadState {
    fn new_woken() -> Arc<Self> {
        Arc::new(Self {
            flags: AtomicU8::new(0xff),
            parent: Mutex::new(None),
        })
    }

    fn clone_parent(&self) -> Option<Waker> {
        let guard = self.parent.lock().unwrap();
        guard.as_ref().cloned()
    }

    fn into_parts(self) -> (AtomicU8, Option<Waker>) {
        let ProcessReadState { flags, parent } = self;

        let parent = parent.lock().unwrap().take();

        (flags, parent)
    }
}

impl ProcessRead {
    fn get_waker(&self, flag: u8) -> Option<Waker> {
        let mask = 0xff ^ flag;
        let previous = self.state.flags.fetch_and(mask, Ordering::AcqRel);
        let active = previous & flag;

        if active == flag {
            Some(
                Arc::new(ProcessReadWaker {
                    state: self.state.clone(),
                    flag,
                })
                .into(),
            )
        } else {
            None
        }
    }

    fn set_parent_waker(&self, parent: &Waker) -> bool {
        let mut guard = self.state.parent.lock().unwrap();
        if let Some(waker) = guard.as_mut() {
            if !waker.will_wake(parent) {
                *waker = parent.clone();
                true
            } else {
                false
            }
        } else {
            *guard = Some(parent.clone());
            true
        }
    }

    fn mark_all_woken(&self) {
        self.state.flags.store(0xff, Ordering::Release);
    }
}

const HANDLE_WAKER: u8 = 0b_0100;

impl AsyncRead for ProcessRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let command = self.command.clone();
        let id = self.id;
        let span = self
            .span
            .get_or_insert_with(|| tracing::info_span!("process task", %command, %id))
            .clone();
        let guard = span.enter();

        let value = loop {
            // always poll for bytes when poll_read is called
            let before_size = buf.filled().len();

            if let Poll::Ready(res) = Pin::new(&mut self.stdout).poll_read(cx, buf) {
                if let Err(e) = res {
                    self.closed = true;

                    break Poll::Ready(Err(e));
                } else if buf.filled().len() == before_size {
                    self.closed = true;

                    break Poll::Ready(Ok(()));
                } else {
                    break Poll::Ready(Ok(()));
                }
            } else if self.closed {
                // Stop if we're closed
                break Poll::Ready(Ok(()));
            } else if let Some(waker) = self.get_waker(HANDLE_WAKER) {
                // only poll handle if we've been explicitly woken
                let mut handle_cx = Context::from_waker(&waker);

                if let Poll::Ready(res) = Pin::new(&mut self.handle).poll(&mut handle_cx) {
                    self.closed = true;

                    if let Err(e) = res {
                        break Poll::Ready(Err(e));
                    }
                }
            } else if self.set_parent_waker(cx.waker()) {
                // if we updated the stored waker, mark all as woken an try polling again
                // This doesn't actually "wake" the waker, it just allows the handle to be polled
                // again next iteration
                self.mark_all_woken();
            } else {
                // if the waker hasn't changed and nothing polled ready, return pending
                break Poll::Pending;
            }
        };

        drop(guard);

        value
    }
}

impl Wake for ProcessReadWaker {
    fn wake(self: Arc<Self>) {
        match Arc::try_unwrap(self) {
            Ok(ProcessReadWaker { state, flag }) => match Arc::try_unwrap(state) {
                Ok(state) => {
                    let (flags, parent) = state.into_parts();

                    flags.fetch_and(flag, Ordering::AcqRel);

                    if let Some(parent) = parent {
                        parent.wake();
                    }
                }
                Err(state) => {
                    state.flags.fetch_or(flag, Ordering::AcqRel);

                    if let Some(waker) = state.clone_parent() {
                        waker.wake();
                    }
                }
            },
            Err(this) => this.wake_by_ref(),
        }
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.state.flags.fetch_or(self.flag, Ordering::AcqRel);

        if let Some(parent) = self.state.clone_parent() {
            parent.wake();
        }
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Command failed with bad status: {}", self.0)
    }
}

impl std::error::Error for StatusError {}
