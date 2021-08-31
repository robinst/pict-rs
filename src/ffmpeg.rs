#[derive(Debug, thiserror::Error)]
pub(crate) enum VideoError {
    #[error("Failed to interface with transcode process")]
    IO(#[from] std::io::Error),

    #[error("Failed to convert file")]
    Status,

    #[error("Transcode semaphore is closed")]
    Closed,
}

pub(crate) enum InputFormat {
    Gif,
    Mp4,
}

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

static MAX_TRANSCODES: once_cell::sync::OnceCell<tokio::sync::Semaphore> =
    once_cell::sync::OnceCell::new();

fn semaphore() -> &'static tokio::sync::Semaphore {
    MAX_TRANSCODES
        .get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get().saturating_sub(1).max(1)))
}

pub(crate) fn to_mp4_stream<S, E>(
    input: S,
    input_format: InputFormat,
) -> std::io::Result<futures::stream::LocalBoxStream<'static, Result<actix_web::web::Bytes, E>>>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E>> + Unpin + 'static,
    E: From<std::io::Error> + 'static,
{
    let process = crate::stream::Process::spawn(tokio::process::Command::new("ffmpeg").args([
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
    ]))?;

    Ok(Box::pin(process.sink_stream(input).unwrap()))
}

pub(crate) async fn thumbnail<P1, P2>(
    from: P1,
    to: P2,
    format: ThumbnailFormat,
) -> Result<(), VideoError>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let mut child = tokio::process::Command::new("ffmpeg")
        .arg(&"-i")
        .arg(&from.as_ref())
        .args([
            "-vframes",
            "1",
            "-codec",
            format.as_codec(),
            "-f",
            format.as_format(),
        ])
        .arg(&to.as_ref())
        .spawn()?;

    let status = child.wait().await?;
    drop(permit);

    if !status.success() {
        return Err(VideoError::Status);
    }

    Ok(())
}

impl From<tokio::sync::AcquireError> for VideoError {
    fn from(_: tokio::sync::AcquireError) -> VideoError {
        VideoError::Closed
    }
}
