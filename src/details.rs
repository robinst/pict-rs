use crate::{
    discover::DiscoveryLite,
    error::Error,
    formats::{InternalFormat, InternalVideoFormat},
    serde_str::Serde,
    store::Store,
};
use actix_web::web;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct HumanDate {
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) timestamp: time::OffsetDateTime,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Details {
    width: u16,
    height: u16,
    frames: Option<u32>,
    content_type: Serde<mime::Mime>,
    created_at: HumanDate,
    format: InternalFormat,
}

impl Details {
    pub(crate) fn is_video(&self) -> bool {
        self.content_type.type_() == "video"
    }

    pub(crate) fn created_at(&self) -> time::OffsetDateTime {
        self.created_at.timestamp
    }

    pub(crate) async fn from_bytes(timeout: u64, input: web::Bytes) -> Result<Self, Error> {
        let DiscoveryLite {
            format,
            width,
            height,
            frames,
        } = crate::discover::discover_bytes_lite(timeout, input).await?;

        Ok(Details::from_parts(format, width, height, frames))
    }

    pub(crate) async fn from_store<S: Store>(
        store: &S,
        identifier: &S::Identifier,
        timeout: u64,
    ) -> Result<Self, Error> {
        let DiscoveryLite {
            format,
            width,
            height,
            frames,
        } = crate::discover::discover_store_lite(store, identifier, timeout).await?;

        Ok(Details::from_parts(format, width, height, frames))
    }

    pub(crate) fn internal_format(&self) -> InternalFormat {
        self.format
    }

    pub(crate) fn media_type(&self) -> mime::Mime {
        (*self.content_type).clone()
    }

    pub(crate) fn system_time(&self) -> std::time::SystemTime {
        self.created_at.into()
    }

    pub(crate) fn video_format(&self) -> Option<InternalVideoFormat> {
        match self.format {
            InternalFormat::Video(format) => Some(format),
            _ => None,
        }
    }

    pub(crate) fn from_parts_full(
        format: InternalFormat,
        width: u16,
        height: u16,
        frames: Option<u32>,
        created_at: HumanDate,
    ) -> Self {
        Self {
            width,
            height,
            frames,
            content_type: Serde::new(format.media_type()),
            created_at,
            format,
        }
    }

    pub(crate) fn from_parts(
        format: InternalFormat,
        width: u16,
        height: u16,
        frames: Option<u32>,
    ) -> Self {
        Self {
            width,
            height,
            frames,
            content_type: Serde::new(format.media_type()),
            created_at: HumanDate {
                timestamp: OffsetDateTime::now_utc(),
            },
            format,
        }
    }
}

impl From<HumanDate> for std::time::SystemTime {
    fn from(HumanDate { timestamp }: HumanDate) -> Self {
        timestamp.into()
    }
}

impl std::fmt::Display for HumanDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self
            .timestamp
            .format(&Rfc3339)
            .map_err(|_| std::fmt::Error)?;

        f.write_str(&s)
    }
}
