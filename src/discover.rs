mod exiftool;
mod ffmpeg;
mod magick;

use actix_web::web::Bytes;

use crate::{
    formats::{InputFile, InternalFormat},
    store::Store,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Discovery {
    pub(crate) input: InputFile,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) frames: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DiscoveryLite {
    pub(crate) format: InternalFormat,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) frames: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DiscoverError {
    #[error("No frames in uploaded media")]
    NoFrames,

    #[error("Not all frames have same image format")]
    FormatMismatch,

    #[error("Input file type {0} is unsupported")]
    UnsupportedFileType(String),
}

pub(crate) async fn discover_bytes_lite(
    bytes: Bytes,
) -> Result<DiscoveryLite, crate::error::Error> {
    if let Some(discovery) = ffmpeg::discover_bytes_lite(bytes.clone()).await? {
        return Ok(discovery);
    }

    let discovery = magick::discover_bytes_lite(bytes).await?;

    Ok(discovery)
}

pub(crate) async fn discover_store_lite<S>(
    store: &S,
    identifier: &S::Identifier,
) -> Result<DiscoveryLite, crate::error::Error>
where
    S: Store + 'static,
{
    if let Some(discovery) =
        ffmpeg::discover_stream_lite(store.to_stream(identifier, None, None).await?).await?
    {
        return Ok(discovery);
    }

    let discovery =
        magick::discover_stream_lite(store.to_stream(identifier, None, None).await?).await?;

    Ok(discovery)
}

pub(crate) async fn discover_bytes(bytes: Bytes) -> Result<Discovery, crate::error::Error> {
    let discovery = ffmpeg::discover_bytes(bytes.clone()).await?;

    let discovery = magick::confirm_bytes(discovery, bytes.clone()).await?;

    let discovery = exiftool::check_reorient(discovery, bytes).await?;

    Ok(discovery)
}
