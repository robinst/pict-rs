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

pub(crate) enum ValidFormat {
    Jpeg,
    Png,
    Webp,
}

impl ValidFormat {
    fn as_magic_type(&self) -> &'static str {
        match self {
            ValidFormat::Jpeg => "JPEG",
            ValidFormat::Png => "PNG",
            ValidFormat::Webp => "WEBP",
        }
    }
}

static MAX_CONVERSIONS: once_cell::sync::OnceCell<tokio::sync::Semaphore> =
    once_cell::sync::OnceCell::new();

fn semaphore() -> &'static tokio::sync::Semaphore {
    MAX_CONVERSIONS
        .get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get().saturating_sub(1).max(1)))
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

pub(crate) async fn validate_format<P>(file: &P, format: ValidFormat) -> Result<(), MagickError>
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

    if s.lines()
        .all(|item| item.is_empty() || item == format.as_magic_type())
    {
        return Ok(());
    }

    Err(MagickError::Format)
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
