mod exiftool;
mod ffmpeg;
mod magick;

use actix_web::web::Bytes;

use crate::{formats::InputFile, tmp_file::TmpDir};

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

pub(crate) async fn discover_bytes(
    tmp_dir: &TmpDir,
    timeout: u64,
    bytes: Bytes,
) -> Result<Discovery, crate::error::Error> {
    let discovery = ffmpeg::discover_bytes(tmp_dir, timeout, bytes.clone()).await?;

    let discovery = magick::confirm_bytes(tmp_dir, discovery, timeout, bytes.clone()).await?;

    let discovery = exiftool::check_reorient(discovery, timeout, bytes).await?;

    Ok(discovery)
}
