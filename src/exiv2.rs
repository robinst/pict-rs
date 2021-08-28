#[derive(Debug, thiserror::Error)]
pub(crate) enum Exvi2Error {
    #[error("Failed to interface with exiv2")]
    IO(#[from] std::io::Error),

    #[error("Mime Parse: {0}")]
    Mime(#[from] mime::FromStrError),

    #[error("Identify semaphore is closed")]
    Closed,

    #[error("Requested information is not present")]
    Missing,

    #[error("Exiv2 command failed")]
    Status,

    #[error("Requested information was present, but not supported")]
    Unsupported,
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

static MAX_READS: once_cell::sync::OnceCell<tokio::sync::Semaphore> =
    once_cell::sync::OnceCell::new();

fn semaphore() -> &'static tokio::sync::Semaphore {
    MAX_READS.get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get() * 4))
}

pub(crate) async fn clear_metadata<P>(file: P) -> Result<(), Exvi2Error>
where
    P: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let status = tokio::process::Command::new("exiv2")
        .arg(&"rm")
        .arg(&file.as_ref())
        .spawn()?
        .wait()
        .await?;

    drop(permit);

    if !status.success() {
        return Err(Exvi2Error::Status);
    }

    Ok(())
}

pub(crate) async fn input_type<P>(file: P) -> Result<ValidInputType, Exvi2Error>
where
    P: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let output = tokio::process::Command::new("exiv2")
        .arg(&"pr")
        .arg(&file.as_ref())
        .output()
        .await?;
    drop(permit);

    let s = String::from_utf8_lossy(&output.stdout);

    let mime_line = s
        .lines()
        .find(|line| line.starts_with("MIME"))
        .ok_or_else(|| Exvi2Error::Missing)?;

    let mut segments = mime_line.rsplit(':');
    let mime_type = segments.next().ok_or_else(|| Exvi2Error::Missing)?;

    let input_type = match mime_type.trim() {
        "video/mp4" => ValidInputType::Mp4,
        "video/quicktime" => ValidInputType::Mp4,
        "image/gif" => ValidInputType::Gif,
        "image/png" => ValidInputType::Png,
        "image/jpeg" => ValidInputType::Jpeg,
        "image/webp" => ValidInputType::Webp,
        _ => return Err(Exvi2Error::Unsupported),
    };

    Ok(input_type)
}

pub(crate) async fn details<P>(file: P) -> Result<Details, Exvi2Error>
where
    P: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let output = tokio::process::Command::new("exiv2")
        .arg(&"pr")
        .arg(&file.as_ref())
        .output()
        .await?;
    drop(permit);

    let s = String::from_utf8_lossy(&output.stdout);
    parse_output(s)
}

fn parse_output(s: std::borrow::Cow<'_, str>) -> Result<Details, Exvi2Error> {
    let mime_line = s
        .lines()
        .find(|line| line.starts_with("MIME"))
        .ok_or_else(|| Exvi2Error::Missing)?;

    let mut segments = mime_line.rsplit(':');
    let mime_type = segments.next().ok_or_else(|| Exvi2Error::Missing)?.trim();

    let resolution_line = s
        .lines()
        .find(|line| line.starts_with("Image size"))
        .ok_or_else(|| Exvi2Error::Missing)?;

    let mut segments = resolution_line.rsplit(':');
    let resolution = segments.next().ok_or_else(|| Exvi2Error::Missing)?;
    let mut resolution_segments = resolution.split('x');
    let width_str = resolution_segments
        .next()
        .ok_or_else(|| Exvi2Error::Missing)?
        .trim();
    let height_str = resolution_segments
        .next()
        .ok_or_else(|| Exvi2Error::Missing)?
        .trim();

    let width = width_str.parse()?;
    let height = height_str.parse()?;

    Ok(Details {
        mime_type: mime_type.parse()?,
        width,
        height,
    })
}

impl From<tokio::sync::AcquireError> for Exvi2Error {
    fn from(_: tokio::sync::AcquireError) -> Exvi2Error {
        Exvi2Error::Closed
    }
}

impl From<std::num::ParseIntError> for Exvi2Error {
    fn from(_: std::num::ParseIntError) -> Exvi2Error {
        Exvi2Error::Unsupported
    }
}
