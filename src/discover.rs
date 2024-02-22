mod exiftool;
mod ffmpeg;
mod magick;

use actix_web::web::Bytes;

use crate::{bytes_stream::BytesStream, formats::InputFile, state::State};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Discovery {
    pub(crate) input: InputFile,
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

#[tracing::instrument(level = "trace", skip_all)]
pub(crate) async fn discover_bytes_stream<S>(
    state: &State<S>,
    bytes: BytesStream,
) -> Result<Discovery, crate::error::Error> {
    let discovery = ffmpeg::discover_bytes_stream(state, bytes.clone()).await?;

    let discovery = magick::confirm_bytes_stream(state, discovery, bytes.clone()).await?;

    let discovery =
        exiftool::check_reorient(discovery, bytes, state.config.media.process_timeout).await?;

    Ok(discovery)
}
