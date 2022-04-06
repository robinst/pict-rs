use std::ops::Deref;
use time::{Duration, OffsetDateTime};

use crate::CONFIG;

const SECONDS: i64 = 1;
const MINUTES: i64 = 60 * SECONDS;
const HOURS: i64 = 60 * MINUTES;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize)]
pub(super) struct DateTime {
    #[serde(with = "time::serde::rfc3339")]
    inner_date: OffsetDateTime,
}

impl DateTime {
    pub(super) fn now() -> Self {
        DateTime {
            inner_date: OffsetDateTime::now_utc(),
        }
    }

    pub(super) fn min_cache_date(&self) -> Self {
        let cache_duration = Duration::new(CONFIG.media.cache_duration * HOURS, 0);

        Self {
            inner_date: self
                .checked_sub(cache_duration)
                .expect("Should never be older than Jan 7, 1970"),
        }
    }
}

impl std::fmt::Display for DateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner_date.fmt(f)
    }
}

impl AsRef<OffsetDateTime> for DateTime {
    fn as_ref(&self) -> &OffsetDateTime {
        &self.inner_date
    }
}

impl Deref for DateTime {
    type Target = OffsetDateTime;

    fn deref(&self) -> &Self::Target {
        &self.inner_date
    }
}

impl From<OffsetDateTime> for DateTime {
    fn from(inner_date: OffsetDateTime) -> Self {
        Self { inner_date }
    }
}
