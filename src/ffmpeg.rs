#[derive(Debug, thiserror::Error)]
pub(crate) enum VideoError {
    #[error("Failed to interface with transcode process")]
    IO(#[from] std::io::Error),

    #[error("Failed to convert file")]
    Status,

    #[error("Transcode semaphore is closed")]
    Closed,
}

static MAX_TRANSCODES: once_cell::sync::OnceCell<tokio::sync::Semaphore> =
    once_cell::sync::OnceCell::new();

fn semaphore() -> &'static tokio::sync::Semaphore {
    MAX_TRANSCODES
        .get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get().saturating_sub(1).max(1)))
}

pub(crate) async fn thumbnail_jpeg<P1, P2>(from: P1, to: P2) -> Result<(), VideoError>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    thumbnail(from, to, "mjpeg").await
}

pub(crate) async fn thumbnail_png<P1, P2>(from: P1, to: P2) -> Result<(), VideoError>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    thumbnail(from, to, "png").await
}

pub(crate) async fn to_mp4<P1, P2>(from: P1, to: P2) -> Result<(), VideoError>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let mut child = tokio::process::Command::new("ffmpeg")
        .args([
            &AsRef::<std::ffi::OsStr>::as_ref(&"-i"),
            &from.as_ref().as_ref(),
            &"-movflags".as_ref(),
            &"faststart".as_ref(),
            &"-pix_fmt".as_ref(),
            &"yuv420p".as_ref(),
            &"-vf".as_ref(),
            &"scale=trunc(iw/2)*2:truc(ih/2)*2".as_ref(),
            &"-an".as_ref(),
            &"-codec".as_ref(),
            &"h264".as_ref(),
            &to.as_ref().as_ref(),
        ])
        .spawn()?;

    let status = child.wait().await?;
    drop(permit);

    if !status.success() {
        return Err(VideoError::Status);
    }

    Ok(())
}

async fn thumbnail<P1, P2>(from: P1, to: P2, codec: &str) -> Result<(), VideoError>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let mut child = tokio::process::Command::new("ffmpeg")
        .args([
            &AsRef::<std::ffi::OsStr>::as_ref(&"-i"),
            &from.as_ref().as_ref(),
            &"-ss".as_ref(),
            &"00:00:01.000".as_ref(),
            &"-vframes".as_ref(),
            &"1".as_ref(),
            &"-codec".as_ref(),
            &codec.as_ref(),
            &to.as_ref().as_ref(),
        ])
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
