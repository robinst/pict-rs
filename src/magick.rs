use crate::{config::Format, stream::Process};
use actix_web::web::Bytes;
use std::process::Stdio;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("{0}")]
    IO(#[from] std::io::Error),

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

pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::spawn(Command::new("magick").args(["convert", "-", "-strip", "-"]))?;

    Ok(process.bytes_read(input).unwrap())
}

pub(crate) async fn details_bytes(input: Bytes) -> Result<Details, MagickError> {
    let process = Process::spawn(Command::new("magick").args([
        "identify",
        "-ping",
        "-format",
        "%w %h | %m\n",
        "-",
    ]))?;

    let mut reader = process.bytes_read(input).unwrap();

    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    let s = String::from_utf8_lossy(&bytes);

    parse_details(s)
}

pub(crate) fn convert_bytes_read(
    input: Bytes,
    format: Format,
) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::spawn(Command::new("magick").args([
        "convert",
        "-",
        format!("{}:-", format.to_magick_format()).as_str(),
    ]))?;

    Ok(process.bytes_read(input).unwrap())
}

pub(crate) async fn details<P>(file: P) -> Result<Details, MagickError>
where
    P: AsRef<std::path::Path>,
{
    let output = Command::new("magick")
        .args([&"identify", &"-ping", &"-format", &"%w %h | %m\n"])
        .arg(&file.as_ref())
        .output()
        .await?;

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

pub(crate) async fn input_type_bytes(mut input: Bytes) -> Result<ValidInputType, MagickError> {
    let mut child = Command::new("magick")
        .args(["identify", "-ping", "-format", "%m\n", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    stdin.write_all_buf(&mut input).await?;
    drop(stdin);

    let mut vec = Vec::new();
    stdout.read_to_end(&mut vec).await?;
    drop(stdout);

    child.wait().await?;

    let s = String::from_utf8_lossy(&vec);
    parse_input_type(s)
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

pub(crate) fn process_image_write_read(
    input: impl AsyncRead + Unpin + 'static,
    args: Vec<String>,
    format: Format,
) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::spawn(
        Command::new("magick")
            .args([&"convert", &"-"])
            .args(args)
            .arg(format!("{}:-", format.to_magick_format())),
    )?;

    Ok(process.write_read(input).unwrap())
}

impl From<std::num::ParseIntError> for MagickError {
    fn from(_: std::num::ParseIntError) -> MagickError {
        MagickError::Format
    }
}
