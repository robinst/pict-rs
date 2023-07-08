use crate::{
    error::Error,
    ffmpeg::VideoFormat,
    magick::{video_mp4, video_webm, ValidInputType},
    serde_str::Serde,
    store::Store,
};
use actix_web::web;
use time::format_description::well_known::Rfc3339;

#[derive(Copy, Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum MaybeHumanDate {
    HumanDate(#[serde(with = "time::serde::rfc3339")] time::OffsetDateTime),
    OldDate(#[serde(serialize_with = "time::serde::rfc3339::serialize")] time::OffsetDateTime),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Details {
    width: usize,
    height: usize,
    frames: Option<usize>,
    content_type: Serde<mime::Mime>,
    created_at: MaybeHumanDate,
}

impl Details {
    pub(crate) fn is_motion(&self) -> bool {
        self.content_type.type_() == "video"
            || self.content_type.type_() == "image" && self.content_type.subtype() == "gif"
    }

    pub(crate) async fn from_bytes(input: web::Bytes, hint: ValidInputType) -> Result<Self, Error> {
        let details = if hint.is_video() {
            crate::ffmpeg::details_bytes(input.clone()).await?
        } else {
            None
        };

        let details = if let Some(details) = details {
            details
        } else {
            crate::magick::details_bytes(input, Some(hint)).await?
        };

        Ok(Details::now(
            details.width,
            details.height,
            details.mime_type,
            details.frames,
        ))
    }

    pub(crate) async fn from_store<S: Store + 'static>(
        store: S,
        identifier: S::Identifier,
        expected_format: Option<ValidInputType>,
    ) -> Result<Self, Error> {
        let details = if expected_format.map(|t| t.is_video()).unwrap_or(true) {
            crate::ffmpeg::details_store(&store, &identifier).await?
        } else {
            None
        };

        let details = if let Some(details) = details {
            details
        } else {
            crate::magick::details_store(store, identifier, expected_format).await?
        };

        Ok(Details::now(
            details.width,
            details.height,
            details.mime_type,
            details.frames,
        ))
    }

    pub(crate) fn now(
        width: usize,
        height: usize,
        content_type: mime::Mime,
        frames: Option<usize>,
    ) -> Self {
        Details {
            width,
            height,
            frames,
            content_type: Serde::new(content_type),
            created_at: MaybeHumanDate::HumanDate(time::OffsetDateTime::now_utc()),
        }
    }

    pub(crate) fn content_type(&self) -> mime::Mime {
        (*self.content_type).clone()
    }

    pub(crate) fn system_time(&self) -> std::time::SystemTime {
        self.created_at.into()
    }

    pub(crate) fn to_input_format(&self) -> Option<VideoFormat> {
        if *self.content_type == mime::IMAGE_GIF {
            return Some(VideoFormat::Gif);
        }

        if *self.content_type == video_mp4() {
            return Some(VideoFormat::Mp4);
        }

        if *self.content_type == video_webm() {
            return Some(VideoFormat::Webm);
        }

        None
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
