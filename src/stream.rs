use actix_web::web::Bytes;
use futures::stream::{LocalBoxStream, Stream};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

pub(crate) struct ReadAdapter<S> {
    inner: S,
}

pub(crate) struct Process {
    child: tokio::process::Child,
}

pub(crate) struct ProcessRead<I> {
    inner: I,
    err_recv: tokio::sync::oneshot::Receiver<std::io::Error>,
    err_closed: bool,
}

pub(crate) struct ProcessSinkStream<E> {
    stream: LocalBoxStream<'static, Result<Bytes, E>>,
}

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

        actix_rt::spawn(async move {
            if let Err(e) = stdin.write_all_buf(&mut input).await {
                let _ = tx.send(e);
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

        actix_rt::spawn(async move {
            if let Err(e) = tokio::io::copy(&mut input_reader, &mut stdin).await {
                let _ = tx.send(e);
            }
        });

        Some(Box::pin(ProcessRead {
            inner: stdout,
            err_recv: rx,
            err_closed: false,
        }))
    }
}

impl<S, E> AsyncRead for ReadAdapter<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                buf.put_slice(&bytes);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e)))
            }
            Poll::Pending => Poll::Pending,
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
            }
        }

        if let Poll::Ready(res) = Pin::new(&mut self.inner).poll_read(cx, buf) {
            return Poll::Ready(res);
        }

        Poll::Pending
    }
}

impl<E> Stream for ProcessSinkStream<E> {
    type Item = Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.stream).poll_next(cx)
    }
}
