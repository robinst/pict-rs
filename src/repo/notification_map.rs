use dashmap::DashMap;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    time::Duration,
};
use tokio::sync::Notify;

use crate::future::WithTimeout;

type Map = Arc<DashMap<Arc<str>, Weak<NotificationEntryInner>>>;

#[derive(Clone)]
pub(super) struct NotificationMap {
    map: Map,
}

pub(crate) struct NotificationEntry {
    inner: Arc<NotificationEntryInner>,
}

struct NotificationEntryInner {
    key: Arc<str>,
    map: Map,
    notify: Notify,
    armed: AtomicBool,
}

impl NotificationMap {
    pub(super) fn new() -> Self {
        Self {
            map: Arc::new(DashMap::new()),
        }
    }

    pub(super) fn register_interest(&self, key: Arc<str>) -> NotificationEntry {
        let new_entry = Arc::new(NotificationEntryInner {
            key: key.clone(),
            map: self.map.clone(),
            notify: crate::sync::bare_notify(),
            armed: AtomicBool::new(false),
        });

        let mut key_entry = self
            .map
            .entry(key)
            .or_insert_with(|| Arc::downgrade(&new_entry));

        let upgraded_entry = key_entry.value().upgrade();

        let inner = if let Some(entry) = upgraded_entry {
            entry
        } else {
            *key_entry.value_mut() = Arc::downgrade(&new_entry);
            new_entry
        };

        inner.armed.store(true, Ordering::Release);

        NotificationEntry { inner }
    }

    pub(super) fn notify(&self, key: &str) {
        if let Some(notifier) = self.map.get(key).and_then(|v| v.upgrade()) {
            notifier.notify.notify_waiters();
        }
    }
}

impl NotificationEntry {
    pub(crate) fn notified_timeout(
        &mut self,
        duration: Duration,
    ) -> impl Future<Output = Result<(), tokio::time::error::Elapsed>> + '_ {
        self.inner.notify.notified().with_timeout(duration)
    }
}

impl Default for NotificationMap {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NotificationEntryInner {
    fn drop(&mut self) {
        if self.armed.load(Ordering::Acquire) {
            self.map.remove(&self.key);
        }
    }
}
