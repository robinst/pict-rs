use crate::error::UploadError;
use actix_web::web::{Bytes, BytesMut};
use futures_util::Stream;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

#[derive(Debug)]
struct StatusError;

pub(crate) struct Process {
    child: tokio::process::Child,
}

pub(crate) struct ProcessRead<I> {
    inner: I,
    err_recv: tokio::sync::oneshot::Receiver<std::io::Error>,
    err_closed: bool,
}

struct BytesFreezer<S>(S);

impl Process {
    fn new(child: tokio::process::Child) -> Self {
        Process { child }
    }

    pub(crate) fn spawn(cmd: &mut tokio::process::Command) -> std::io::Result<Self> {
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map(Process::new)
    }

    pub(crate) fn bytes_read(mut self, mut input: Bytes) -> Option<impl AsyncRead + Unpin> {
        let mut stdin = self.child.stdin.take()?;
        let stdout = self.child.stdout.take()?;

        let (tx, rx) = tokio::sync::oneshot::channel();

        let mut child = self.child;

        actix_rt::spawn(async move {
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
        }))
    }

    pub(crate) fn write_read(
        mut self,
        mut input_reader: impl AsyncRead + Unpin + 'static,
    ) -> Option<impl AsyncRead + Unpin> {
        let mut stdin = self.child.stdin.take()?;
        let stdout = self.child.stdout.take()?;

        let (tx, rx) = tokio::sync::oneshot::channel();

        let mut child = self.child;

        actix_rt::spawn(async move {
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
        }))
    }
}

pub(crate) fn bytes_stream(
    input: impl AsyncRead + Unpin,
) -> impl Stream<Item = Result<Bytes, UploadError>> + Unpin {
    BytesFreezer(tokio_util::codec::FramedRead::new(
        input,
        tokio_util::codec::BytesCodec::new(),
    ))
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

impl<S> Stream for BytesFreezer<S>
where
    S: Stream<Item = std::io::Result<BytesMut>> + Unpin,
{
    type Item = Result<Bytes, UploadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map(|opt| opt.map(|res| res.map(|bytes_mut| bytes_mut.freeze())))
            .map_err(UploadError::from)
    }
}

impl std::fmt::Display for StatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Command failed with bad status")
    }
}

impl std::error::Error for StatusError {}
