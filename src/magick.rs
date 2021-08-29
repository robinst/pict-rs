#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("{0}")]
    IO(#[from] std::io::Error),

    #[error("Magick command failed")]
    Status,

    #[error("Magick semaphore is closed")]
    Closed,

    #[error("Invalid format")]
    Format,
}

pub(crate) enum ValidInputType {
    Mp4,
    Gif,
    Png,
    Jpeg,
    Webp,
}

pub(crate) struct Details {
    pub(crate) mime_type: mime::Mime,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

static MAX_CONVERSIONS: once_cell::sync::OnceCell<tokio::sync::Semaphore> =
    once_cell::sync::OnceCell::new();

fn semaphore() -> &'static tokio::sync::Semaphore {
    MAX_CONVERSIONS.get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get().max(1) * 5))
}

pub(crate) async fn convert_file<P1, P2>(
    from: P1,
    to: P2,
    format: crate::config::Format,
) -> Result<(), MagickError>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    let mut output_file = std::ffi::OsString::new();
    output_file.extend([format.to_magick_format().as_ref(), ":".as_ref()]);
    output_file.extend([to.as_ref().as_ref()]);

    let permit = semaphore().acquire().await?;

    let status = tokio::process::Command::new("magick")
        .arg("convert")
        .arg(&from.as_ref())
        .arg(&output_file)
        .spawn()?
        .wait()
        .await?;

    drop(permit);

    if !status.success() {
        return Err(MagickError::Status);
    }

    Ok(())
}

pub(crate) async fn details<P>(file: P) -> Result<Details, MagickError>
where
    P: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let output = tokio::process::Command::new("magick")
        .args([&"identify", &"-ping", &"-format", &"%w %h | %m\n"])
        .arg(&file.as_ref())
        .output()
        .await?;

    drop(permit);

    let s = String::from_utf8_lossy(&output.stdout);
    tracing::debug!("lines: {}", s);

    let mut lines = s.lines();
    let first = lines.next().ok_or_else(|| MagickError::Format)?;

    let mut segments = first.split('|');

    let dimensions = segments.next().ok_or_else(|| MagickError::Format)?.trim();
    tracing::debug!("dimensions: {}", dimensions);
    let mut dims = dimensions.split(' ');
    let width = dims
        .next()
        .ok_or_else(|| MagickError::Format)?
        .trim()
        .parse()?;
    let height = dims
        .next()
        .ok_or_else(|| MagickError::Format)?
        .trim()
        .parse()?;

    let format = segments.next().ok_or_else(|| MagickError::Format)?.trim();
    tracing::debug!("format: {}", format);

    if !lines.all(|item| item.ends_with(format)) {
        return Err(MagickError::Format);
    }

    let mime_type = match format {
        "MP4" => crate::validate::video_mp4(),
        "GIF" => mime::IMAGE_GIF,
        "PNG" => mime::IMAGE_PNG,
        "JPEG" => mime::IMAGE_JPEG,
        "WEBP" => crate::validate::image_webp(),
        _ => return Err(MagickError::Format),
    };

    Ok(Details {
        mime_type,
        width,
        height,
    })
}

pub(crate) async fn input_type<P>(file: &P) -> Result<ValidInputType, MagickError>
where
    P: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let output = tokio::process::Command::new("magick")
        .args([&"identify", &"-ping", &"-format", &"%m\n"])
        .arg(&file.as_ref())
        .output()
        .await?;

    drop(permit);

    let s = String::from_utf8_lossy(&output.stdout);

    let mut lines = s.lines();
    let first = lines.next();

    let opt = lines.fold(first, |acc, item| match acc {
        Some(prev) if prev == item => Some(prev),
        _ => None,
    });

    match opt {
        Some("MP4") => Ok(ValidInputType::Mp4),
        Some("GIF") => Ok(ValidInputType::Gif),
        Some("PNG") => Ok(ValidInputType::Png),
        Some("JPEG") => Ok(ValidInputType::Jpeg),
        Some("WEBP") => Ok(ValidInputType::Webp),
        _ => Err(MagickError::Format),
    }
}

pub(crate) async fn process_image<P1, P2>(
    input: P1,
    output: P2,
    args: Vec<String>,
    format: crate::config::Format,
) -> Result<(), MagickError>
where
    P1: AsRef<std::path::Path>,
    P2: AsRef<std::path::Path>,
{
    let mut output_file = std::ffi::OsString::new();
    output_file.extend([format!("{}:", format.to_magick_format()).as_ref()]);
    output_file.extend([output.as_ref().as_ref()]);

    let permit = semaphore().acquire().await?;

    let status = tokio::process::Command::new("magick")
        .arg(&"convert")
        .arg(&input.as_ref())
        .args(args)
        .arg(&output.as_ref())
        .spawn()?
        .wait()
        .await?;

    drop(permit);

    if !status.success() {
        return Err(MagickError::Status);
    }

    Ok(())
}

impl From<tokio::sync::AcquireError> for MagickError {
    fn from(_: tokio::sync::AcquireError) -> MagickError {
        MagickError::Closed
    }
}

impl From<std::num::ParseIntError> for MagickError {
    fn from(_: std::num::ParseIntError) -> MagickError {
        MagickError::Format
    }
}
