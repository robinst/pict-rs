#[derive(Debug, thiserror::Error)]
pub(crate) enum FormatError {
    #[error("Failed to interface with exiv2")]
    IO(#[from] std::io::Error),

    #[error("Failed to identify file")]
    Status,

    #[error("Identify semaphore is closed")]
    Closed,

    #[error("Requested information is not present")]
    Missing,

    #[error("Requested information was present, but not supported")]
    Unsupported,
}

pub(crate) enum ValidInputType {
    Mp4,
    Gif,
    Png,
    Jpeg,
}

static MAX_READS: once_cell::sync::OnceCell<tokio::sync::Semaphore> =
    once_cell::sync::OnceCell::new();

fn semaphore() -> &'static tokio::sync::Semaphore {
    MAX_READS.get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get() * 4))
}

pub(crate) async fn format<P>(file: P) -> Result<ValidInputType, FormatError>
where
    P: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let output = tokio::process::Command::new("exiv2")
        .args([
            &AsRef::<std::ffi::OsStr>::as_ref(&"pr"),
            &file.as_ref().as_ref(),
        ])
        .output()
        .await?;
    drop(permit);

    if !output.status.success() {
        return Err(FormatError::Status);
    }

    let s = String::from_utf8_lossy(&output.stdout);

    let line = s
        .lines()
        .find(|line| line.starts_with("MIME"))
        .ok_or_else(|| FormatError::Missing)?;

    let mut segments = line.rsplit(':');
    let mime_type = segments.next().ok_or_else(|| FormatError::Missing)?;

    let input_type = match mime_type.trim() {
        "video/mp4" => ValidInputType::Mp4,
        "image/gif" => ValidInputType::Gif,
        "image/png" => ValidInputType::Png,
        "image/jpeg" => ValidInputType::Jpeg,
        _ => return Err(FormatError::Unsupported),
    };

    Ok(input_type)
}

impl From<tokio::sync::AcquireError> for FormatError {
    fn from(_: tokio::sync::AcquireError) -> FormatError {
        FormatError::Closed
    }
}
