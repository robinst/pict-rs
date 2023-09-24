use std::sync::Arc;

use tokio::sync::{Notify, Semaphore};

#[track_caller]
pub(crate) fn channel<T>(bound: usize) -> (flume::Sender<T>, flume::Receiver<T>) {
    tracing::trace_span!(parent: None, "make channel").in_scope(|| flume::bounded(bound))
}

#[track_caller]
pub(crate) fn notify() -> Arc<Notify> {
    Arc::new(bare_notify())
}

#[track_caller]
pub(crate) fn bare_notify() -> Notify {
    tracing::trace_span!(parent: None, "make notifier").in_scope(Notify::new)
}

#[track_caller]
pub(crate) fn bare_semaphore(permits: usize) -> Semaphore {
    tracing::trace_span!(parent: None, "make semaphore").in_scope(|| Semaphore::new(permits))
}

#[track_caller]
pub(crate) fn spawn<F>(future: F) -> actix_web::rt::task::JoinHandle<F::Output>
where
    F: std::future::Future + 'static,
    F::Output: 'static,
{
    tracing::trace_span!(parent: None, "spawn task").in_scope(|| actix_web::rt::spawn(future))
}

#[track_caller]
pub(crate) fn spawn_blocking<F, Out>(function: F) -> actix_web::rt::task::JoinHandle<Out>
where
    F: FnOnce() -> Out + Send + 'static,
    Out: Send + 'static,
{
    let outer_span = tracing::Span::current();

    tracing::trace_span!(parent: None, "spawn blocking task")
        .in_scope(|| actix_web::rt::task::spawn_blocking(move || outer_span.in_scope(function)))
}
