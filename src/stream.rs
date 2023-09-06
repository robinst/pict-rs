use actix_rt::{task::JoinHandle, time::Sleep};
use actix_web::web::Bytes;
use flume::r#async::RecvStream;
use futures_core::Stream;
use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

pub(crate) trait MakeSend<T>: Stream<Item = std::io::Result<T>>
where
    T: 'static,
{
    fn make_send(self) -> MakeSendStream<T>
    where
        Self: Sized + 'static,
    {
        let (tx, rx) = crate::sync::channel(4);

        MakeSendStream {
            handle: crate::sync::spawn(async move {
                let this = std::pin::pin!(self);

                let mut stream = this.into_streamer();

                while let Some(res) = stream.next().await {
                    if tx.send_async(res).await.is_err() {
                        return;
                    }
                }
            }),
            rx: rx.into_stream(),
        }
    }
}

impl<S, T> MakeSend<T> for S
where
    S: Stream<Item = std::io::Result<T>>,
    T: 'static,
{
}

pub(crate) struct MakeSendStream<T>
where
    T: 'static,
{
    handle: actix_rt::task::JoinHandle<()>,
    rx: flume::r#async::RecvStream<'static, std::io::Result<T>>,
}

impl<T> Stream for MakeSendStream<T>
where
    T: 'static,
{
    type Item = std::io::Result<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.rx).poll_next(cx) {
            Poll::Ready(opt) => Poll::Ready(opt),
            Poll::Pending if std::task::ready!(Pin::new(&mut self.handle).poll(cx)).is_err() => {
                Poll::Ready(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Stream panicked",
                ))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pin_project_lite::pin_project! {
    pub(crate) struct Map<S, F> {
        #[pin]
        stream: S,
        func: F,
    }
}

pub(crate) trait StreamMap: Stream {
    fn map<F, U>(self, func: F) -> Map<Self, F>
    where
        F: FnMut(Self::Item) -> U,
        Self: Sized,
    {
        Map { stream: self, func }
    }
}

impl<T> StreamMap for T where T: Stream {}

impl<S, F, U> Stream for Map<S, F>
where
    S: Stream,
    F: FnMut(S::Item) -> U,
{
    type Item = U;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        let value = std::task::ready!(this.stream.poll_next(cx));

        Poll::Ready(value.map(this.func))
    }
}

pub(crate) struct Empty<T>(PhantomData<T>);

impl<T> Stream for Empty<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

pub(crate) fn empty<T>() -> Empty<T> {
    Empty(PhantomData)
}

pub(crate) struct Once<T>(Option<T>);

impl<T> Stream for Once<T>
where
    T: Unpin,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.take())
    }
}

pub(crate) fn once<T>(value: T) -> Once<T> {
    Once(Some(value))
}

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
        let stream = self.0.as_mut().take()?;

        let opt = std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await;

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

enum IterStreamState<I, T>
where
    T: 'static,
{
    New {
        iterator: I,
        buffer: usize,
    },
    Running {
        handle: JoinHandle<()>,
        receiver: RecvStream<'static, T>,
    },
    Pending,
}

pub(crate) struct IterStream<I, T>
where
    T: 'static,
{
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
                let (sender, receiver) = crate::sync::channel(buffer);

                let mut handle = crate::sync::spawn_blocking(move || {
                    let iterator = iterator.into_iter();

                    for item in iterator {
                        if sender.send(item).is_err() {
                            break;
                        }
                    }
                });

                if Pin::new(&mut handle).poll(cx).is_ready() {
                    return Poll::Ready(None);
                }

                this.state = IterStreamState::Running {
                    handle,
                    receiver: receiver.into_stream(),
                };

                self.poll_next(cx)
            }
            IterStreamState::Running {
                mut handle,
                mut receiver,
            } => match Pin::new(&mut receiver).poll_next(cx) {
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
