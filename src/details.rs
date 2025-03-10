use crate::{
    bytes_stream::BytesStream,
    discover::Discovery,
    error::Error,
    formats::{InternalFormat, InternalVideoFormat},
    serde_str::Serde,
    state::State,
};

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct HumanDate {
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) timestamp: time::OffsetDateTime,
}

#[derive(Debug, serde::Serialize)]
enum ApiFormat {
    Image,
    Animation,
    Video,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ApiDetails {
    width: u16,
    height: u16,
    frames: Option<u32>,
    content_type: Serde<mime::Mime>,
    created_at: HumanDate,
    format: ApiFormat,
}

#[derive(Clone, Debug)]
pub(crate) struct Details {
    pub(crate) inner: DetailsInner,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct DetailsInner {
    width: u16,
    height: u16,
    frames: Option<u32>,
    content_type: Serde<mime::Mime>,
    created_at: HumanDate,
    format: InternalFormat,
}

impl Details {
    pub(crate) fn into_api_details(self) -> ApiDetails {
        let Details {
            inner:
                DetailsInner {
                    width,
                    height,
                    frames,
                    content_type,
                    created_at,
                    format,
                },
        } = self;

        ApiDetails {
            width,
            height,
            frames,
            content_type,
            created_at,
            format: match format {
                InternalFormat::Image(_) => ApiFormat::Image,
                InternalFormat::Animation(_) => ApiFormat::Animation,
                InternalFormat::Video(_) => ApiFormat::Video,
            },
        }
    }

    pub(crate) fn created_at(&self) -> time::OffsetDateTime {
        self.inner.created_at.timestamp
    }

    #[tracing::instrument(level = "debug", skip_all)]
    pub(crate) async fn from_bytes_stream<S>(
        state: &State<S>,
        input: BytesStream,
    ) -> Result<Self, Error> {
        let Discovery {
            input,
            width,
            height,
            frames,
        } = crate::discover::discover_bytes_stream(state, input).await?;

        Ok(Details::from_parts(
            input.internal_format(),
            width,
            height,
            frames,
        ))
    }

    pub(crate) fn width(&self) -> u16 {
        self.inner.width
    }

    pub(crate) fn height(&self) -> u16 {
        self.inner.height
    }

    pub(crate) fn internal_format(&self) -> InternalFormat {
        self.inner.format
    }

    pub(crate) fn media_type(&self) -> mime::Mime {
        (*self.inner.content_type).clone()
    }

    pub(crate) fn file_extension(&self) -> &'static str {
        self.inner.format.file_extension()
    }

    pub(crate) fn system_time(&self) -> std::time::SystemTime {
        self.inner.created_at.into()
    }

    pub(crate) fn is_video(&self) -> bool {
        matches!(self.inner.format, InternalFormat::Video(_))
    }

    pub(crate) fn video_format(&self) -> Option<InternalVideoFormat> {
        match self.inner.format {
            InternalFormat::Video(format) => Some(format),
            _ => None,
        }
    }

    pub(crate) fn danger_dummy(format: InternalFormat) -> Self {
        Self::from_parts_full(
            format,
            0,
            0,
            None,
            HumanDate {
                timestamp: time::OffsetDateTime::now_utc(),
            },
        )
    }

    pub(crate) fn from_parts_full(
        format: InternalFormat,
        width: u16,
        height: u16,
        frames: Option<u32>,
        created_at: HumanDate,
    ) -> Self {
        Self {
            inner: DetailsInner {
                width,
                height,
                frames,
                content_type: Serde::new(format.media_type()),
                created_at,
                format,
            },
        }
    }

    pub(crate) fn from_parts(
        format: InternalFormat,
        width: u16,
        height: u16,
        frames: Option<u32>,
    ) -> Self {
        Self {
            inner: DetailsInner {
                width,
                height,
                frames,
                content_type: Serde::new(format.media_type()),
                created_at: HumanDate {
                    timestamp: OffsetDateTime::now_utc(),
                },
                format,
            },
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
