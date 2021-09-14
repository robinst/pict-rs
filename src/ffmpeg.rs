use crate::{
    error::{Error, UploadError},
    stream::Process,
};
use actix_web::web::Bytes;
use tokio::{io::AsyncRead, process::Command};
use tracing::instrument;

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

#[instrument(name = "Create video thumbnail", skip(from, to))]
pub(crate) async fn thumbnail<P1, P2>(
    from: P1,
    to: P2,
    format: ThumbnailFormat,
) -> Result<(), Error>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    let command = "ffmpeg";
    let first_arg = "-i";
    let args = [
        "-vframes",
        "1",
        "-codec",
        format.as_codec(),
        "-f",
        format.as_format(),
    ];

    tracing::info!(
        "Spawning command: {} {} {:?} {:?} {:?}",
        command,
        first_arg,
        from.as_ref(),
        args,
        to.as_ref()
    );

    let mut child = Command::new(command)
        .arg(first_arg)
        .arg(from.as_ref())
        .args(args)
        .arg(to.as_ref())
        .spawn()?;

    let status = child.wait().await?;

    if !status.success() {
        return Err(UploadError::Status.into());
    }

    Ok(())
}
