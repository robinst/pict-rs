use std::sync::Arc;

use tokio::sync::{Notify, Semaphore};

pub(crate) fn channel<T>(bound: usize) -> (flume::Sender<T>, flume::Receiver<T>) {
    tracing::trace_span!(parent: None, "make channel").in_scope(|| flume::bounded(bound))
}

pub(crate) fn notify() -> Arc<Notify> {
    Arc::new(bare_notify())
}

pub(crate) fn bare_notify() -> Notify {
    tracing::trace_span!(parent: None, "make notifier").in_scope(Notify::new)
}

pub(crate) fn bare_semaphore(permits: usize) -> Semaphore {
    tracing::trace_span!(parent: None, "make semaphore").in_scope(|| Semaphore::new(permits))
}

pub(crate) fn spawn<F>(future: F) -> actix_rt::task::JoinHandle<F::Output>
where
    F: std::future::Future + 'static,
    F::Output: 'static,
{
    tracing::trace_span!(parent: None, "spawn task").in_scope(|| actix_rt::spawn(future))
}

pub(crate) fn spawn_blocking<F, Out>(function: F) -> actix_rt::task::JoinHandle<Out>
where
    F: FnOnce() -> Out + Send + 'static,
    Out: Send + 'static,
{
    let outer_span = tracing::Span::current();

    tracing::trace_span!(parent: None, "spawn blocking task")
        .in_scope(|| actix_rt::task::spawn_blocking(move || outer_span.in_scope(function)))
}
