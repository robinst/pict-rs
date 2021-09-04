use actix_web::web::Bytes;
use futures::stream::{LocalBoxStream, Stream, StreamExt};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio_stream::wrappers::ReceiverStream;

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

pub(crate) struct ProcessSink {
    stdin: tokio::process::ChildStdin,
}

pub(crate) struct ProcessStream {
    stream: LocalBoxStream<'static, std::io::Result<Bytes>>,
}

pub(crate) struct ProcessSinkStream<E> {
    stream: LocalBoxStream<'static, Result<Bytes, E>>,
}

pub(crate) struct TryDuplicateStream<T, E> {
    inner: ReceiverStream<Result<T, E>>,
}

#[derive(Debug)]
pub(crate) struct StringError(String);

impl<S> ReadAdapter<S> {
    pub(crate) fn new_unsync<E>(
        mut stream: S,
    ) -> ReadAdapter<ReceiverStream<Result<Bytes, StringError>>>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + 'static,
        E: std::fmt::Display,
    {
        let (tx, rx) = tokio::sync::mpsc::channel(1);

        actix_rt::spawn(async move {
            while let Some(res) = stream.next().await {
                if tx
                    .send(res.map_err(|e| StringError(e.to_string())))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        ReadAdapter::new_sync(ReceiverStream::new(rx))
    }

    fn new_sync<E>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        ReadAdapter { inner: stream }
    }
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

    pub(crate) fn take_sink(&mut self) -> Option<ProcessSink> {
        self.child.stdin.take().map(ProcessSink::new)
    }

    pub(crate) fn take_stream(&mut self) -> Option<ProcessStream> {
        self.child.stdout.take().map(ProcessStream::new)
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

    pub(crate) fn sink_stream<S, E>(mut self, input_stream: S) -> Option<ProcessSinkStream<E>>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + 'static,
        E: From<std::io::Error> + 'static,
    {
        let mut stdin = self.take_sink()?;
        let mut stdout = self.take_stream()?;

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        actix_rt::spawn(async move {
            if let Err(e) = stdin.send(input_stream).await {
                let _ = tx.send(e).await;
            }
        });

        Some(ProcessSinkStream {
            stream: Box::pin(async_stream::stream! {
                loop {
                    tokio::select! {
                        opt = rx.recv() => {
                            if let Some(e) = opt {
                                yield Err(e);
                                break;
                            }
                        }
                        res = stdout.next() => {
                            match res {
                                Some(Ok(bytes)) => yield Ok(bytes),
                                Some(Err(e)) => {
                                    yield Err(e.into());
                                    break;
                                }
                                None => break,
                            }
                        }
                    }
                }

                drop(stdout);
                match self.child.wait().await {
                    Ok(status) if status.success() => return,
                    Ok(_) => yield Err(std::io::Error::from(std::io::ErrorKind::Other).into()),
                    Err(e) => yield Err(e.into()),
                }
            }),
        })
    }
}

impl ProcessSink {
    fn new(stdin: tokio::process::ChildStdin) -> Self {
        ProcessSink { stdin }
    }

    pub(crate) async fn send<S, E>(&mut self, mut stream: S) -> Result<(), E>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: From<std::io::Error>,
    {
        while let Some(res) = stream.next().await {
            let mut bytes = res?;

            self.stdin.write_all_buf(&mut bytes).await?;
        }

        Ok(())
    }
}

impl ProcessStream {
    fn new(mut stdout: tokio::process::ChildStdout) -> ProcessStream {
        let s = async_stream::stream! {
            loop {
                let mut buf = actix_web::web::BytesMut::with_capacity(65_536);

                match stdout.read_buf(&mut buf).await {
                    Ok(len) if len == 0 => {
                        break;
                    }
                    Ok(_) => {
                        yield Ok(buf.freeze());
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        };

        ProcessStream {
            stream: Box::pin(s),
        }
    }
}

pub(crate) fn try_duplicate<S, T, E>(
    mut stream: S,
    buffer: usize,
) -> (impl Stream<Item = Result<T, E>>, TryDuplicateStream<T, E>)
where
    S: Stream<Item = Result<T, E>> + Unpin,
    T: Clone,
{
    let (tx, rx) = tokio::sync::mpsc::channel(buffer);
    let s = async_stream::stream! {
        while let Some(value) = stream.next().await {
            match value {
                Ok(t) => {
                    let _ = tx.send(Ok(t.clone())).await;
                    yield Ok(t);
                }
                Err(e) => yield Err(e),
            }
        }
    };

    (
        s,
        TryDuplicateStream {
            inner: ReceiverStream::new(rx),
        },
    )
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

impl Stream for ProcessStream {
    type Item = std::io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.stream).poll_next(cx)
    }
}

impl<E> Stream for ProcessSinkStream<E> {
    type Item = Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.stream).poll_next(cx)
    }
}

impl<T, E> Stream for TryDuplicateStream<T, E> {
    type Item = Result<T, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl std::fmt::Display for StringError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StringError {}
