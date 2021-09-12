use crate::{config::Format, stream::Process};
use actix_web::web::Bytes;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("{0}")]
    IO(#[from] std::io::Error),

    #[error("Invalid format")]
    Format,

    #[error("Image too large")]
    Dimensions,
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
    let first = lines.next().ok_or(MagickError::Format)?;

    let mut segments = first.split('|');

    let dimensions = segments.next().ok_or(MagickError::Format)?.trim();
    tracing::debug!("dimensions: {}", dimensions);
    let mut dims = dimensions.split(' ');
    let width = dims.next().ok_or(MagickError::Format)?.trim().parse()?;
    let height = dims.next().ok_or(MagickError::Format)?.trim().parse()?;

    let format = segments.next().ok_or(MagickError::Format)?.trim();
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

pub(crate) async fn input_type_bytes(input: Bytes) -> Result<ValidInputType, MagickError> {
    details_bytes(input).await?.validate_input()
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

impl Details {
    fn validate_input(&self) -> Result<ValidInputType, MagickError> {
        if self.width > crate::CONFIG.max_width() || self.height > crate::CONFIG.max_height() {
            return Err(MagickError::Dimensions);
        }

        let input_type = match (self.mime_type.type_(), self.mime_type.subtype()) {
            (mime::VIDEO, mime::MP4 | mime::MPEG) => ValidInputType::Mp4,
            (mime::IMAGE, mime::GIF) => ValidInputType::Gif,
            (mime::IMAGE, mime::PNG) => ValidInputType::Png,
            (mime::IMAGE, mime::JPEG) => ValidInputType::Jpeg,
            (mime::IMAGE, subtype) if subtype.as_str() == "webp" => ValidInputType::Webp,
            _ => return Err(MagickError::Format),
        };

        Ok(input_type)
    }
}

impl From<std::num::ParseIntError> for MagickError {
    fn from(_: std::num::ParseIntError) -> MagickError {
        MagickError::Format
    }
}
