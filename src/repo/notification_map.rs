use dashmap::{mapref::entry::Entry, DashMap};
use std::{
    future::Future,
    sync::{Arc, Weak},
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
}

impl NotificationMap {
    pub(super) fn new() -> Self {
        Self {
            map: Arc::new(DashMap::new()),
        }
    }

    pub(super) fn register_interest(&self, key: Arc<str>) -> NotificationEntry {
        match self.map.entry(key.clone()) {
            Entry::Occupied(mut occupied) => {
                if let Some(inner) = occupied.get().upgrade() {
                    NotificationEntry { inner }
                } else {
                    let inner = Arc::new(NotificationEntryInner {
                        key,
                        map: self.map.clone(),
                        notify: crate::sync::bare_notify(),
                    });

                    occupied.insert(Arc::downgrade(&inner));

                    NotificationEntry { inner }
                }
            }
            Entry::Vacant(vacant) => {
                let inner = Arc::new(NotificationEntryInner {
                    key,
                    map: self.map.clone(),
                    notify: crate::sync::bare_notify(),
                });

                vacant.insert(Arc::downgrade(&inner));

                NotificationEntry { inner }
            }
        }
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
        self.map.remove(&self.key);
    }
}
