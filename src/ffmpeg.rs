use crate::{error::Error, process::Process, store::Store};
use actix_web::web::Bytes;
use tokio::io::AsyncRead;
use tracing::instrument;

#[derive(Debug)]
pub(crate) enum InputFormat {
    Gif,
    Mp4,
}

#[derive(Debug)]
pub(crate) enum ThumbnailFormat {
    Jpeg,
    // Webp,
}

impl InputFormat {
    fn as_format(&self) -> &'static str {
        match self {
            InputFormat::Gif => "gif_pipe",
            InputFormat::Mp4 => "mp4",
        }
    }
}

impl ThumbnailFormat {
    fn as_codec(&self) -> &'static str {
        match self {
            ThumbnailFormat::Jpeg => "mjpeg",
            // ThumbnailFormat::Webp => "webp",
        }
    }

    fn as_format(&self) -> &'static str {
        match self {
            ThumbnailFormat::Jpeg => "singlejpeg",
            // ThumbnailFormat::Webp => "webp",
        }
    }
}

pub(crate) fn to_mp4_bytes(
    input: Bytes,
    input_format: InputFormat,
) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::run(
        "ffmpeg",
        &[
            "-f",
            input_format.as_format(),
            "-i",
            "pipe:",
            "-movflags",
            "faststart+frag_keyframe+empty_moov",
            "-pix_fmt",
            "yuv420p",
            "-vf",
            "scale=trunc(iw/2)*2:trunc(ih/2)*2",
            "-an",
            "-codec",
            "h264",
            "-f",
            "mp4",
            "pipe:",
        ],
    )?;

    Ok(process.bytes_read(input).unwrap())
}

#[instrument(name = "Create video thumbnail")]
pub(crate) async fn thumbnail<S: Store>(
    store: S,
    from: S::Identifier,
    input_format: InputFormat,
    format: ThumbnailFormat,
) -> Result<impl AsyncRead + Unpin, Error> {
    let process = Process::run(
        "ffmpeg",
        &[
            "-f",
            input_format.as_format(),
            "-i",
            "pipe:",
            "-vframes",
            "1",
            "-codec",
            format.as_codec(),
            "-f",
            format.as_format(),
            "pipe:",
        ],
    )?;

    Ok(process.store_read(store, from).unwrap())
}
