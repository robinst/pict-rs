use actix_rt::{task::JoinHandle, time::Sleep};
use actix_web::web::Bytes;
use futures_util::Stream;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

pub(crate) type LocalBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

pub(crate) trait StreamLimit {
    fn limit(self, limit: u64) -> Limit<Self>
    where
        Self: Sized,
    {
        Limit {
            inner: self,
            count: 0,
            limit,
        }
    }
}

pub(crate) trait StreamTimeout {
    fn timeout(self, duration: Duration) -> Timeout<Self>
    where
        Self: Sized,
    {
        Timeout {
            sleep: actix_rt::time::sleep(duration),
            inner: self,
            expired: false,
            woken: Arc::new(AtomicBool::new(true)),
        }
    }
}

pub(crate) trait IntoStreamer: Stream {
    fn into_streamer(self) -> Streamer<Self>
    where
        Self: Sized,
    {
        Streamer(Some(self))
    }
}

impl<T> IntoStreamer for T where T: Stream + Unpin {}

pub(crate) fn from_iterator<I: IntoIterator + Unpin + Send + 'static>(
    iterator: I,
    buffer: usize,
) -> IterStream<I, I::Item> {
    IterStream {
        state: IterStreamState::New { iterator, buffer },
    }
}

impl<S, E> StreamLimit for S where S: Stream<Item = Result<Bytes, E>> {}
impl<S> StreamTimeout for S where S: Stream {}

pub(crate) struct Streamer<S>(Option<S>);

impl<S> Streamer<S> {
    pub(crate) async fn next(&mut self) -> Option<S::Item>
    where
        S: Stream + Unpin,
    {
        let opt = match self.0 {
            Some(ref mut stream) => {
                std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
            }
            None => None,
        };

        if opt.is_none() {
            self.0.take();
        }

        opt
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct Limit<S> {
        #[pin]
        inner: S,

        count: u64,
        limit: u64,
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct Timeout<S> {
        #[pin]
        sleep: Sleep,

        #[pin]
        inner: S,

        expired: bool,
        woken: Arc<AtomicBool>,
    }
}

enum IterStreamState<I, T> {
    New {
        iterator: I,
        buffer: usize,
    },
    Running {
        handle: JoinHandle<()>,
        receiver: tokio::sync::mpsc::Receiver<T>,
    },
    Pending,
}

pub(crate) struct IterStream<I, T> {
    state: IterStreamState<I, T>,
}

struct TimeoutWaker {
    woken: Arc<AtomicBool>,
    inner: Waker,
}

#[derive(Debug, thiserror::Error)]
#[error("Resonse body larger than size limit")]
pub(crate) struct LimitError;

#[derive(Debug, thiserror::Error)]
#[error("Timeout in body")]
pub(crate) struct TimeoutError;

impl<S, E> Stream for Limit<S>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: From<LimitError>,
{
    type Item = Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().project();

        let limit = this.limit;
        let count = this.count;
        let inner = this.inner;

        inner.poll_next(cx).map(|opt| {
            opt.map(|res| match res {
                Ok(bytes) => {
                    *count += bytes.len() as u64;
                    if *count > *limit {
                        return Err(LimitError.into());
                    }
                    Ok(bytes)
                }
                Err(e) => Err(e),
            })
        })
    }
}

impl Wake for TimeoutWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref()
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
        self.inner.wake_by_ref();
    }
}

impl<S, T> Stream for Timeout<S>
where
    S: Stream<Item = T>,
{
    type Item = Result<T, TimeoutError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().project();

        if *this.expired {
            return Poll::Ready(None);
        }

        if this.woken.swap(false, Ordering::Acquire) {
            let timeout_waker = Arc::new(TimeoutWaker {
                woken: Arc::clone(this.woken),
                inner: cx.waker().clone(),
            })
            .into();

            let mut timeout_cx = Context::from_waker(&timeout_waker);

            if this.sleep.poll(&mut timeout_cx).is_ready() {
                *this.expired = true;
                return Poll::Ready(Some(Err(TimeoutError)));
            }
        }

        this.inner.poll_next(cx).map(|opt| opt.map(Ok))
    }
}

impl<I, T> Stream for IterStream<I, T>
where
    I: IntoIterator<Item = T> + Send + Unpin + 'static,
    T: Send + 'static,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();

        match std::mem::replace(&mut this.state, IterStreamState::Pending) {
            IterStreamState::New { iterator, buffer } => {
                let (sender, receiver) = tracing::trace_span!(parent: None, "Create channel")
                    .in_scope(|| tokio::sync::mpsc::channel(buffer));

                let mut handle = actix_rt::task::spawn_blocking(move || {
                    let iterator = iterator.into_iter();

                    for item in iterator {
                        if sender.blocking_send(item).is_err() {
                            break;
                        }
                    }
                });

                if Pin::new(&mut handle).poll(cx).is_ready() {
                    return Poll::Ready(None);
                }

                this.state = IterStreamState::Running { handle, receiver };

                self.poll_next(cx)
            }
            IterStreamState::Running {
                mut handle,
                mut receiver,
            } => match Pin::new(&mut receiver).poll_recv(cx) {
                Poll::Ready(Some(item)) => {
                    this.state = IterStreamState::Running { handle, receiver };

                    Poll::Ready(Some(item))
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => {
                    if Pin::new(&mut handle).poll(cx).is_ready() {
                        return Poll::Ready(None);
                    }

                    this.state = IterStreamState::Running { handle, receiver };

                    Poll::Pending
                }
            },
            IterStreamState::Pending => panic!("Polled after completion"),
        }
    }
}
