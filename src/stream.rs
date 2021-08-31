use actix_web::web::Bytes;
use futures::stream::{LocalBoxStream, Stream, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) struct Process {
    child: tokio::process::Child,
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
    inner: tokio_stream::wrappers::ReceiverStream<Result<T, E>>,
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
            inner: tokio_stream::wrappers::ReceiverStream::new(rx),
        },
    )
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
