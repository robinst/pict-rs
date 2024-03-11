use actix_web::web::Bytes;
use futures_core::Stream;
use std::{pin::Pin, time::Duration};
use streem::IntoStreamer;

use crate::future::WithMetrics;

#[cfg(not(feature = "random-errors"))]
pub(crate) fn error_injector(
    stream: impl Stream<Item = std::io::Result<Bytes>>,
) -> impl Stream<Item = std::io::Result<Bytes>> {
    stream
}

#[cfg(feature = "random-errors")]
pub(crate) fn error_injector(
    stream: impl Stream<Item = std::io::Result<Bytes>>,
) -> impl Stream<Item = std::io::Result<Bytes>> {
    streem::try_from_fn(|yielder| async move {
        let stream = std::pin::pin!(stream);
        let mut streamer = stream.into_streamer();

        while let Some(item) = streamer.try_next().await? {
            yielder.yield_ok(item).await;

            use nanorand::Rng;

            if nanorand::tls_rng().generate_range(0..1000) < 1 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    crate::error::UploadError::RandomError,
                ));
            }
        }

        Ok(())
    })
}

pub(crate) fn take<S>(stream: S, amount: usize) -> impl Stream<Item = S::Item>
where
    S: Stream,
    S::Item: 'static,
{
    streem::from_fn(|yielder| async move {
        let stream = std::pin::pin!(stream);
        let mut streamer = stream.into_streamer();

        let mut count = 0;

        if count == amount {
            return;
        }

        while let Some(item) = streamer.next().await {
            tracing::trace!("take: looping");

            yielder.yield_(item).await;

            count += 1;

            if count == amount {
                break;
            }
        }
    })
}

pub(crate) fn metrics<S>(name: &'static str, stream: S) -> impl Stream<Item = S::Item>
where
    S: Stream,
    S::Item: 'static,
{
    streem::from_fn(|yielder| {
        async move {
            let stream = std::pin::pin!(stream);
            let mut streamer = stream.into_streamer();

            while let Some(item) = streamer.next().await {
                tracing::trace!("metrics: looping");

                yielder.yield_(item).await;
            }
        }
        .with_metrics(name)
    })
}

pub(crate) fn make_send<S>(stream: S) -> impl Stream<Item = S::Item> + Send
where
    S: Stream + 'static,
    S::Item: Send + Sync,
{
    let (tx, rx) = crate::sync::channel(1);

    let handle = crate::sync::abort_on_drop(crate::sync::spawn("send-stream", async move {
        let stream = std::pin::pin!(stream);
        let mut streamer = stream.into_streamer();

        while let Some(res) = streamer.next().await {
            tracing::trace!("make send tx: looping");

            if tx.send_async(res).await.is_err() {
                break;
            }
        }
    }));

    streem::from_fn(|yiedler| async move {
        let mut stream = rx.into_stream().into_streamer();

        while let Some(res) = stream.next().await {
            tracing::trace!("make send rx: looping");

            yiedler.yield_(res).await;
        }

        let _ = handle.await;
    })
}

pub(crate) fn from_iterator<I>(iterator: I, buffer: usize) -> impl Stream<Item = I::Item> + Send
where
    I: IntoIterator + Send + 'static,
    I::Item: Send + Sync,
{
    let (tx, rx) = crate::sync::channel(buffer);

    let handle = crate::sync::spawn_blocking("blocking-iterator", move || {
        for value in iterator {
            if tx.send(value).is_err() {
                break;
            }
        }
    });

    streem::from_fn(|yielder| async move {
        let mut stream = rx.into_stream().into_streamer();

        while let Some(res) = stream.next().await {
            tracing::trace!("from_iterator: looping");

            yielder.yield_(res).await;

            crate::sync::cooperate().await;
        }

        let _ = handle.await;
    })
}

pub(crate) fn map<S, I1, I2, F>(stream: S, f: F) -> impl Stream<Item = I2>
where
    S: Stream<Item = I1>,
    I2: 'static,
    F: Fn(I1) -> I2 + Copy,
{
    streem::from_fn(|yielder| async move {
        let stream = std::pin::pin!(stream);
        let mut streamer = stream.into_streamer();

        while let Some(res) = streamer.next().await {
            tracing::trace!("map: looping");

            yielder.yield_((f)(res)).await;
        }
    })
}

#[cfg(not(feature = "io-uring"))]
pub(crate) fn map_ok<S, T1, T2, E, F>(stream: S, f: F) -> impl Stream<Item = Result<T2, E>>
where
    S: Stream<Item = Result<T1, E>>,
    T2: 'static,
    E: 'static,
    F: Fn(T1) -> T2 + Copy,
{
    map(stream, move |res| res.map(f))
}

pub(crate) fn map_err<S, T, E1, E2, F>(stream: S, f: F) -> impl Stream<Item = Result<T, E2>>
where
    S: Stream<Item = Result<T, E1>>,
    T: 'static,
    E2: 'static,
    F: Fn(E1) -> E2 + Copy,
{
    map(stream, move |res| res.map_err(f))
}

pub(crate) fn from_err<S, T, E1, E2>(stream: S) -> impl Stream<Item = Result<T, E2>>
where
    S: Stream<Item = Result<T, E1>>,
    T: 'static,
    E1: Into<E2>,
    E2: 'static,
{
    map_err(stream, Into::into)
}

pub(crate) fn empty<T>() -> impl Stream<Item = T>
where
    T: 'static,
{
    streem::from_fn(|_| std::future::ready(()))
}

pub(crate) fn timeout<S>(
    duration: Duration,
    stream: S,
) -> impl Stream<Item = Result<S::Item, TimeoutError>>
where
    S: Stream,
    S::Item: 'static,
{
    streem::try_from_fn(|yielder| async move {
        tokio::time::timeout(duration, async move {
            let stream = std::pin::pin!(stream);
            let mut streamer = stream.into_streamer();

            while let Some(res) = streamer.next().await {
                tracing::trace!("timeout: looping");

                yielder.yield_ok(res).await;
            }
        })
        .await
        .map_err(|_| TimeoutError)
    })
}

pub(crate) fn limit<S, E>(limit: usize, stream: S) -> impl Stream<Item = Result<Bytes, E>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: From<LimitError> + 'static,
{
    streem::try_from_fn(|yielder| async move {
        let stream = std::pin::pin!(stream);
        let mut streamer = stream.into_streamer();

        let mut count = 0;

        while let Some(bytes) = streamer.try_next().await? {
            tracing::trace!("limit: looping");

            count += bytes.len();

            if count > limit {
                return Err(LimitError.into());
            }

            yielder.yield_ok(bytes).await;
        }

        Ok(())
    })
}

pub(crate) type LocalBoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + 'a>>;

#[derive(Debug, thiserror::Error)]
#[error("Resonse body larger than size limit")]
pub(crate) struct LimitError;

#[derive(Debug, thiserror::Error)]
#[error("Timeout in body")]
pub(crate) struct TimeoutError;
