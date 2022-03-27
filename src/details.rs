use crate::{error::Error, magick::ValidInputType, serde_str::Serde, store::Store};
use actix_web::web;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Details {
    width: usize,
    height: usize,
    content_type: Serde<mime::Mime>,
    created_at: time::OffsetDateTime,
}

impl Details {
    pub(crate) fn is_motion(&self) -> bool {
        self.content_type.type_() == "video"
            || self.content_type.type_() == "image" && self.content_type.subtype() == "gif"
    }

    #[tracing::instrument("Details from bytes", skip(input))]
    pub(crate) async fn from_bytes(
        input: web::Bytes,
        hint: Option<ValidInputType>,
    ) -> Result<Self, Error> {
        let details = crate::magick::details_bytes(input, hint).await?;

        Ok(Details::now(
            details.width,
            details.height,
            details.mime_type,
        ))
    }

    #[tracing::instrument("Details from store")]
    pub(crate) async fn from_store<S: Store>(
        store: S,
        identifier: S::Identifier,
        expected_format: Option<ValidInputType>,
    ) -> Result<Self, Error> {
        let details = crate::magick::details_store(store, identifier, expected_format).await?;

        Ok(Details::now(
            details.width,
            details.height,
            details.mime_type,
        ))
    }

    pub(crate) fn now(width: usize, height: usize, content_type: mime::Mime) -> Self {
        Details {
            width,
            height,
            content_type: Serde::new(content_type),
            created_at: time::OffsetDateTime::now_utc(),
        }
    }

    pub(crate) fn content_type(&self) -> mime::Mime {
        (*self.content_type).clone()
    }

    pub(crate) fn system_time(&self) -> std::time::SystemTime {
        self.created_at.into()
    }
}
