#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("{0}")]
    IO(#[from] std::io::Error),

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
    MAX_CONVERSIONS
        .get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get().saturating_sub(1).max(1)))
}

pub(crate) fn clear_metadata_stream<S, E>(
    input: S,
) -> std::io::Result<futures::stream::LocalBoxStream<'static, Result<actix_web::web::Bytes, E>>>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E>> + Unpin + 'static,
    E: From<std::io::Error> + 'static,
{
    let process = crate::stream::Process::spawn(
        tokio::process::Command::new("magick").args(["convert", "-", "-strip", "-"]),
    )?;

    Ok(Box::pin(process.sink_stream(input).unwrap()))
}

pub(crate) fn convert_stream<S, E>(
    input: S,
    format: crate::config::Format,
) -> std::io::Result<futures::stream::LocalBoxStream<'static, Result<actix_web::web::Bytes, E>>>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E>> + Unpin + 'static,
    E: From<std::io::Error> + 'static,
{
    let process = crate::stream::Process::spawn(tokio::process::Command::new("magick").args([
        "convert",
        "-",
        format!("{}:-", format.to_magick_format()).as_str(),
    ]))?;

    Ok(Box::pin(process.sink_stream(input).unwrap()))
}

pub(crate) async fn details_stream<S, E1, E2>(input: S) -> Result<Details, E2>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E1>> + Unpin,
    E1: From<std::io::Error>,
    E2: From<E1> + From<std::io::Error> + From<MagickError>,
{
    use futures::stream::StreamExt;

    let permit = semaphore().acquire().await.map_err(MagickError::from)?;

    let mut process =
        crate::stream::Process::spawn(tokio::process::Command::new("magick").args([
            "identify",
            "-ping",
            "-format",
            "%w %h | %m\n",
            "-",
        ]))?;

    process.take_sink().unwrap().send(input).await?;
    let mut stream = process.take_stream().unwrap();

    let mut buf = actix_web::web::BytesMut::new();
    while let Some(res) = stream.next().await {
        let bytes = res?;
        buf.extend_from_slice(&bytes);
    }

    drop(permit);

    let s = String::from_utf8_lossy(&buf);
    Ok(parse_details(s)?)
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

    parse_details(s)
}

fn parse_details(s: std::borrow::Cow<'_, str>) -> Result<Details, MagickError> {
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

pub(crate) async fn input_type_stream<S, E1, E2>(input: S) -> Result<ValidInputType, E2>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E1>> + Unpin,
    E1: From<std::io::Error>,
    E2: From<E1> + From<std::io::Error> + From<MagickError>,
{
    use futures::stream::StreamExt;

    let permit = semaphore().acquire().await.map_err(MagickError::from)?;

    let mut process = crate::stream::Process::spawn(
        tokio::process::Command::new("magick").args(["identify", "-ping", "-format", "%m\n", "-"]),
    )?;

    process.take_sink().unwrap().send(input).await?;
    let mut stream = process.take_stream().unwrap();

    let mut buf = actix_web::web::BytesMut::new();
    while let Some(res) = stream.next().await {
        let bytes = res?;
        buf.extend_from_slice(&bytes);
    }

    drop(permit);

    let s = String::from_utf8_lossy(&buf);

    Ok(parse_input_type(s)?)
}

fn parse_input_type(s: std::borrow::Cow<'_, str>) -> Result<ValidInputType, MagickError> {
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

pub(crate) fn process_image_stream<S, E>(
    input: S,
    args: Vec<String>,
    format: crate::config::Format,
) -> std::io::Result<futures::stream::LocalBoxStream<'static, Result<actix_web::web::Bytes, E>>>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E>> + Unpin + 'static,
    E: From<std::io::Error> + 'static,
{
    let process = crate::stream::Process::spawn(
        tokio::process::Command::new("magick")
            .args([&"convert", &"-"])
            .args(args)
            .arg(format!("{}:-", format.to_magick_format())),
    )?;

    Ok(Box::pin(process.sink_stream(input).unwrap()))
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
