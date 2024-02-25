use std::{
    ffi::OsStr,
    future::Future,
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use futures_core::Stream;
use streem::IntoStreamer;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
};
use tokio_util::{bytes::Bytes, io::ReaderStream};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    bytes_stream::BytesStream,
    error_code::ErrorCode,
    future::{LocalBoxFuture, WithTimeout},
    read::BoxRead,
};

struct MetricsGuard {
    start: Instant,
    armed: bool,
    command: Arc<str>,
}

impl MetricsGuard {
    fn guard(command: Arc<str>) -> Self {
        metrics::counter!(crate::init_metrics::PROCESS_START, "command" => command.to_string())
            .increment(1);

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
            crate::init_metrics::PROCESS_DURATION,
            "command" => self.command.to_string(),
            "completed" => (!self.armed).to_string(),
        )
        .record(self.start.elapsed().as_secs_f64());

        metrics::counter!(crate::init_metrics::PROCESS_END, "completed" => (!self.armed).to_string() , "command" => self.command.to_string()).increment(1);
    }
}

#[derive(Debug)]
struct StatusError(ExitStatus);

pub(crate) struct Process {
    command: Arc<str>,
    child: Child,
    guard: MetricsGuard,
    timeout: Duration,
    extras: Box<dyn Extras>,
    id: Uuid,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process").field("child", &"Child").finish()
    }
}

#[async_trait::async_trait(?Send)]
pub(crate) trait Extras {
    async fn consume(&mut self) -> std::io::Result<()>;
}

#[async_trait::async_trait(?Send)]
impl Extras for () {
    async fn consume(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl<T> Extras for (Box<dyn Extras>, T)
where
    T: Extras,
{
    async fn consume(&mut self) -> std::io::Result<()> {
        let (res1, res2) = tokio::join!(self.0.consume(), self.1.consume());
        res1?;
        res2
    }
}

pub(crate) struct ProcessRead {
    reader: BoxRead<'static>,
    handle: LocalBoxFuture<'static, Result<(), ProcessError>>,
    command: Arc<str>,
    id: Uuid,
    extras: Box<dyn Extras>,
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

    #[error("Failed to read stdout for {0}")]
    Read(Arc<str>, #[source] std::io::Error),

    #[error("Failed to cleanup for command {0}")]
    Cleanup(Arc<str>, #[source] std::io::Error),

    #[error("Unknown process error for command {0}")]
    Other(Arc<str>, #[source] std::io::Error),
}

impl ProcessError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::COMMAND_NOT_FOUND,
            Self::PermissionDenied(_) => ErrorCode::COMMAND_PERMISSION_DENIED,
            Self::LimitReached | Self::Read(_, _) | Self::Cleanup(_, _) | Self::Other(_, _) => {
                ErrorCode::COMMAND_ERROR
            }
            Self::Timeout(_) => ErrorCode::COMMAND_TIMEOUT,
            Self::Status(_, _) => ErrorCode::COMMAND_FAILURE,
        }
    }

    pub(crate) fn is_client_error(&self) -> bool {
        matches!(self, Self::Timeout(_))
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
        T: AsRef<OsStr> + std::fmt::Debug,
    {
        let command: Arc<str> = Arc::from(String::from(command));

        tracing::debug!("{envs:?} {command} {args:?}");

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
                _ => Err(ProcessError::Other(command, e)),
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
                extras: Box::new(()),
                id: Uuid::now_v7(),
            })
        })
    }

    pub(crate) fn add_extras(self, extra: impl Extras + 'static) -> Self {
        Self {
            extras: Box::new((self.extras, extra)),
            ..self
        }
    }

    #[tracing::instrument(skip(self), fields(command = %self.command, id = %self.id))]
    pub(crate) async fn wait(self) -> Result<(), ProcessError> {
        let Process {
            command,
            mut child,
            guard,
            timeout,
            mut extras,
            id: _,
        } = self;

        let res = child.wait().with_timeout(timeout).await;

        extras
            .consume()
            .await
            .map_err(|e| ProcessError::Cleanup(command.clone(), e))?;

        match res {
            Ok(Ok(status)) if status.success() => {
                guard.disarm();

                Ok(())
            }
            Ok(Ok(status)) => Err(ProcessError::Status(command, status)),
            Ok(Err(e)) => Err(ProcessError::Other(command, e)),
            Err(_) => {
                let _ = child.kill().await;
                Err(ProcessError::Timeout(command))
            }
        }
    }

    pub(crate) fn drive_with_async_read(self, input: impl AsyncRead + 'static) -> ProcessRead {
        self.drive(move |mut stdin| {
            async move {
                let mut input = std::pin::pin!(input);

                match tokio::io::copy(&mut input, &mut stdin).await {
                    Ok(_) => Ok(()),
                    // BrokenPipe means we finished reading from Stdout, so we don't need to write
                    // to stdin. We'll still error out if the command failed so treat this as a
                    // success
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                    Err(e) => Err(e),
                }
            }
        })
    }

    pub(crate) fn drive_with_stream<S>(self, input: S) -> ProcessRead
    where
        S: Stream<Item = std::io::Result<Bytes>> + 'static,
    {
        self.drive(move |mut stdin| async move {
            let stream = std::pin::pin!(input);
            let mut stream = stream.into_streamer();

            while let Some(mut bytes) = stream.try_next().await? {
                match stdin.write_all_buf(&mut bytes).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
                    Err(e) => return Err(e),
                }
            }

            Ok(())
        })
    }

    pub(crate) fn read(self) -> ProcessRead {
        self.drive(|_| async { Ok(()) })
    }

    #[allow(unknown_lints)]
    #[allow(clippy::let_with_type_underscore)]
    #[tracing::instrument(level = "trace", skip_all)]
    fn drive<F, Fut>(self, f: F) -> ProcessRead
    where
        F: FnOnce(ChildStdin) -> Fut + 'static,
        Fut: Future<Output = std::io::Result<()>>,
    {
        let Process {
            command,
            mut child,
            guard,
            timeout,
            extras,
            id,
        } = self;

        let stdin = child.stdin.take().expect("stdin exists");
        let stdout = child.stdout.take().expect("stdout exists");

        let command2 = command.clone();
        let handle = Box::pin(async move {
            let child_fut = async {
                (f)(stdin).await?;

                child.wait().await
            };

            match child_fut.with_timeout(timeout).await {
                Ok(Ok(status)) if status.success() => {
                    guard.disarm();
                    Ok(())
                }
                Ok(Ok(status)) => Err(ProcessError::Status(command2, status)),
                Ok(Err(e)) => Err(ProcessError::Other(command2, e)),
                Err(_) => {
                    child
                        .kill()
                        .await
                        .map_err(|e| ProcessError::Other(command2.clone(), e))?;
                    Err(ProcessError::Timeout(command2))
                }
            }
        });

        ProcessRead {
            reader: Box::pin(stdout),
            handle,
            command,
            id,
            extras,
        }
    }
}

impl ProcessRead {
    pub(crate) fn new(reader: BoxRead<'static>, command: Arc<str>, id: Uuid) -> Self {
        Self {
            reader,
            handle: Box::pin(async { Ok(()) }),
            command,
            id,
            extras: Box::new(()),
        }
    }

    pub(crate) async fn into_bytes_stream(self) -> Result<BytesStream, ProcessError> {
        let cmd = self.command.clone();

        self.with_stdout(move |stdout| {
            BytesStream::try_from_stream(ReaderStream::with_capacity(stdout, 1024 * 64))
        })
        .await?
        .map_err(move |e| ProcessError::Read(cmd, e))
    }

    pub(crate) async fn into_vec(self) -> Result<Vec<u8>, ProcessError> {
        let cmd = self.command.clone();

        self.with_stdout(move |mut stdout| async move {
            let mut vec = Vec::new();

            stdout
                .read_to_end(&mut vec)
                .await
                .map_err(|e| ProcessError::Read(cmd, e))
                .map(move |_| vec)
        })
        .await?
    }

    pub(crate) async fn into_string(self) -> Result<String, ProcessError> {
        let cmd = self.command.clone();

        self.with_stdout(move |mut stdout| async move {
            let mut s = String::new();

            stdout
                .read_to_string(&mut s)
                .await
                .map_err(|e| ProcessError::Read(cmd, e))
                .map(move |_| s)
        })
        .await?
    }

    pub(crate) fn pipe(self, process: Process) -> ProcessRead {
        let Process {
            command,
            mut child,
            guard,
            timeout,
            extras,
            id,
        } = process;

        let mut stdin = child.stdin.take().expect("stdin exists");
        let stdout = child.stdout.take().expect("stdout exists");

        let command2 = command.clone();
        let handle = Box::pin(async move {
            self.with_stdout(move |mut stdout| async move {
                let child_fut = async {
                    let n = tokio::io::copy(&mut stdout, &mut stdin).await?;
                    drop(stdout);
                    drop(stdin);

                    child.wait().await
                };

                match child_fut.with_timeout(timeout).await {
                    Ok(Ok(status)) if status.success() => {
                        guard.disarm();
                        Ok(())
                    }
                    Ok(Ok(status)) => Err(ProcessError::Status(command2, status)),
                    Ok(Err(e)) => Err(ProcessError::Other(command2, e)),
                    Err(_) => {
                        child
                            .kill()
                            .await
                            .map_err(|e| ProcessError::Other(command2.clone(), e))?;
                        Err(ProcessError::Timeout(command2))
                    }
                }
            })
            .await?
        });

        ProcessRead {
            reader: Box::pin(stdout),
            handle,
            command,
            id,
            extras,
        }
    }

    pub(crate) async fn with_stdout<Fut>(
        self,
        f: impl FnOnce(BoxRead<'static>) -> Fut,
    ) -> Result<Fut::Output, ProcessError>
    where
        Fut: Future,
    {
        let Self {
            reader,
            handle,
            command,
            id,
            mut extras,
        } = self;

        let (out, res) = tokio::join!(
            (f)(reader).instrument(tracing::info_span!("cmd-reader", %command, %id)),
            handle.instrument(tracing::info_span!("cmd-handle", %command, %id))
        );

        extras
            .consume()
            .await
            .map_err(|e| ProcessError::Cleanup(command, e))?;

        res?;

        Ok(out)
    }

    pub(crate) fn add_extras<E: Extras + 'static>(self, more_extras: E) -> ProcessRead {
        let Self {
            reader,
            handle,
            command,
            id,
            extras,
        } = self;

        Self {
            reader,
            handle,
            command,
            id,
            extras: Box::new((extras, more_extras)),
        }
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Command failed with bad status: {}", self.0)
    }
}

impl std::error::Error for StatusError {}
