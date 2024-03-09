use std::sync::Arc;

use tokio::{
    sync::{Notify, Semaphore},
    task::JoinHandle,
};

use crate::future::WithPollTimer;

pub(crate) struct DropHandle<T> {
    handle: JoinHandle<T>,
}

pub(crate) fn abort_on_drop<T>(handle: JoinHandle<T>) -> DropHandle<T> {
    DropHandle { handle }
}

impl<T> DropHandle<T> {
    pub(crate) fn abort(&self) {
        self.handle.abort();
    }
}

impl<T> Drop for DropHandle<T> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl<T> std::future::Future for DropHandle<T> {
    type Output = <JoinHandle<T> as std::future::Future>::Output;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.handle).poll(cx)
    }
}

#[track_caller]
pub(crate) fn channel<T>(bound: usize) -> (flume::Sender<T>, flume::Receiver<T>) {
    let span = tracing::trace_span!(parent: None, "make channel");
    let guard = span.enter();

    let channel = flume::bounded(bound);

    drop(guard);
    channel
}

#[track_caller]
pub(crate) fn notify() -> Arc<Notify> {
    Arc::new(bare_notify())
}

#[track_caller]
pub(crate) fn bare_notify() -> Notify {
    let span = tracing::trace_span!(parent: None, "make notifier");
    let guard = span.enter();

    let notify = Notify::new();

    drop(guard);
    notify
}

#[track_caller]
pub(crate) fn bare_semaphore(permits: usize) -> Semaphore {
    let span = tracing::trace_span!(parent: None, "make semaphore");
    let guard = span.enter();

    let semaphore = Semaphore::new(permits);

    drop(guard);
    semaphore
}

#[track_caller]
pub(crate) fn spawn<F>(name: &'static str, future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + 'static,
    F::Output: 'static,
{
    let future = future.with_poll_timer(name);

    let span = tracing::trace_span!(parent: None, "spawn task");
    let guard = span.enter();

    #[cfg(tokio_unstable)]
    let handle = tokio::task::Builder::new()
        .name(name)
        .spawn_local(future)
        .expect("Failed to spawn");
    #[cfg(not(tokio_unstable))]
    let handle = tokio::task::spawn_local(future);

    drop(guard);
    handle
}

#[track_caller]
pub(crate) fn spawn_blocking<F, Out>(name: &str, function: F) -> tokio::task::JoinHandle<Out>
where
    F: FnOnce() -> Out + Send + 'static,
    Out: Send + 'static,
{
    #[cfg(not(tokio_unstable))]
    let _ = name;

    let outer_span = tracing::Span::current();

    let span = tracing::trace_span!(parent: None, "spawn blocking task");
    let guard = span.enter();

    #[cfg(tokio_unstable)]
    let handle = tokio::task::Builder::new()
        .name(name)
        .spawn_blocking(move || outer_span.in_scope(function))
        .expect("Failed to spawn");
    #[cfg(not(tokio_unstable))]
    let handle = tokio::task::spawn_blocking(move || outer_span.in_scope(function));

    drop(guard);
    handle
}
