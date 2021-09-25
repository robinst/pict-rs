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
use tracing::Instrument;
use tracing::Span;

#[derive(Debug)]
struct StatusError;

pub(crate) struct Process {
    child: Child,
    span: Span,
}

pub(crate) struct ProcessRead<I> {
    inner: I,
    span: Span,
    err_recv: Receiver<std::io::Error>,
    err_closed: bool,
    handle: JoinHandle<()>,
}

struct BytesFreezer<S>(S, Span);

impl Process {
    pub(crate) fn run(command: &str, args: &[&str]) -> std::io::Result<Self> {
        Self::spawn(Command::new(command).args(args))
    }

    fn spawn_span(&self) -> Span {
        let span = tracing::info_span!(parent: None, "Spawned command writer",);

        span.follows_from(self.span.clone());

        span
    }

    pub(crate) fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        let cmd = cmd.stdin(Stdio::piped()).stdout(Stdio::piped());

        let span = tracing::info_span!(
            "Spawning Command",
            command = &tracing::field::debug(&cmd),
            exception.message = &tracing::field::Empty,
            exception.details = &tracing::field::Empty,
        );
        cmd.spawn().map(|child| Process { child, span })
    }

    pub(crate) fn bytes_read(mut self, mut input: Bytes) -> Option<impl AsyncRead + Unpin> {
        let mut stdin = self.child.stdin.take()?;
        let stdout = self.child.stdout.take()?;

        let (tx, rx) = channel();

        let span = self.spawn_span();
        let mut child = self.child;
        let handle = actix_rt::spawn(
            async move {
                if let Err(e) = stdin.write_all_buf(&mut input).await {
                    let _ = tx.send(e);
                    return;
                }
                drop(stdin);

                match child.wait().await {
                    Ok(status) => {
                        if !status.success() {
                            let _ = tx
                                .send(std::io::Error::new(std::io::ErrorKind::Other, &StatusError));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(e);
                    }
                }
            }
            .instrument(span),
        );

        Some(Box::pin(ProcessRead {
            inner: stdout,
            span: self.span,
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

        let span = self.spawn_span();
        let mut child = self.child;
        let handle = actix_rt::spawn(
            async move {
                if let Err(e) = tokio::io::copy(&mut input_reader, &mut stdin).await {
                    let _ = tx.send(e);
                    return;
                }
                drop(stdin);

                match child.wait().await {
                    Ok(status) => {
                        if !status.success() {
                            let _ = tx
                                .send(std::io::Error::new(std::io::ErrorKind::Other, &StatusError));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(e);
                    }
                }
            }
            .instrument(span),
        );

        Some(Box::pin(ProcessRead {
            inner: stdout,
            span: self.span,
            err_recv: rx,
            err_closed: false,
            handle,
        }))
    }
}

pub(crate) fn bytes_stream(
    input: impl AsyncRead + Unpin,
) -> impl Stream<Item = Result<Bytes, Error>> + Unpin {
    BytesFreezer(
        FramedRead::new(input, BytesCodec::new()),
        tracing::info_span!("Serving bytes from AsyncRead"),
    )
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
        let span = self.span.clone();
        span.in_scope(|| {
            if !self.err_closed {
                if let Poll::Ready(res) = Pin::new(&mut self.err_recv).poll(cx) {
                    self.err_closed = true;
                    if let Ok(err) = res {
                        let display = format!("{}", err);
                        let debug = format!("{:?}", err);
                        span.record("exception.message", &display.as_str());
                        span.record("exception.details", &debug.as_str());
                        return Poll::Ready(Err(err));
                    }
                }
            }

            if let Poll::Ready(res) = Pin::new(&mut self.inner).poll_read(cx, buf) {
                if let Err(err) = &res {
                    let display = format!("{}", err);
                    let debug = format!("{:?}", err);
                    span.record("exception.message", &display.as_str());
                    span.record("exception.details", &debug.as_str());
                }
                return Poll::Ready(res);
            }

            Poll::Pending
        })
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
        let span = self.1.clone();
        span.in_scope(|| {
            Pin::new(&mut self.0)
                .poll_next(cx)
                .map(|opt| opt.map(|res| res.map(|bytes_mut| bytes_mut.freeze())))
                .map_err(Error::from)
        })
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Command failed with bad status")
    }
}

impl std::error::Error for StatusError {}
