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
#[serde(untagged)]
pub(crate) enum MaybeHumanDate {
    HumanDate(#[serde(with = "time::serde::rfc3339")] time::OffsetDateTime),
    OldDate(#[serde(serialize_with = "time::serde::rfc3339::serialize")] time::OffsetDateTime),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Details {
    width: u16,
    height: u16,
    frames: Option<u32>,
    content_type: Serde<mime::Mime>,
    created_at: MaybeHumanDate,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<InternalFormat>,
}

impl Details {
    pub(crate) fn is_video(&self) -> bool {
        self.content_type.type_() == "video"
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

    pub(crate) fn internal_format(&self) -> Option<InternalFormat> {
        if let Some(format) = self.format {
            return Some(format);
        }

        InternalFormat::maybe_from_media_type(&self.content_type, self.frames.is_some())
    }

    pub(crate) fn media_type(&self) -> mime::Mime {
        (*self.content_type).clone()
    }

    pub(crate) fn system_time(&self) -> std::time::SystemTime {
        self.created_at.into()
    }

    pub(crate) fn video_format(&self) -> Option<InternalVideoFormat> {
        if *self.content_type == crate::formats::mimes::video_mp4() {
            return Some(InternalVideoFormat::Mp4);
        }

        if *self.content_type == crate::formats::mimes::video_webm() {
            return Some(InternalVideoFormat::Webm);
        }

        None
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
            created_at: MaybeHumanDate::HumanDate(OffsetDateTime::now_utc()),
            format: Some(format),
        }
    }
}

impl From<MaybeHumanDate> for std::time::SystemTime {
    fn from(this: MaybeHumanDate) -> Self {
        match this {
            MaybeHumanDate::OldDate(old) => old.into(),
            MaybeHumanDate::HumanDate(human) => human.into(),
        }
    }
}

impl std::fmt::Display for MaybeHumanDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OldDate(date) | Self::HumanDate(date) => {
                let s = date.format(&Rfc3339).map_err(|_| std::fmt::Error)?;

                f.write_str(&s)
            }
        }
    }
}
