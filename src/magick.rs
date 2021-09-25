use crate::{
    config::Format,
    error::{Error, UploadError},
    stream::Process,
};
use actix_web::web::Bytes;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
use tracing::instrument;

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
    let process = Process::run("magick", &["convert", "-", "-strip", "-"])?;

    Ok(process.bytes_read(input).unwrap())
}

pub(crate) async fn details_bytes(input: Bytes) -> Result<Details, Error> {
    let process = Process::run(
        "magick",
        &["identify", "-ping", "-format", "%w %h | %m\n", "-"],
    )?;

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
    let process = Process::run(
        "magick",
        &[
            "convert",
            "-",
            format!("{}:-", format.to_magick_format()).as_str(),
        ],
    )?;

    Ok(process.bytes_read(input).unwrap())
}

pub(crate) async fn details<P>(file: P) -> Result<Details, Error>
where
    P: AsRef<std::path::Path>,
{
    let command = "magick";
    let args = ["identify", "-ping", "-format", "%w %h | %m\n"];
    let last_arg = file.as_ref();

    tracing::info!("Spawning command: {} {:?} {:?}", command, args, last_arg);
    let output = Command::new(command)
        .args(args)
        .arg(last_arg)
        .output()
        .await?;

    let s = String::from_utf8_lossy(&output.stdout);

    parse_details(s)
}

fn parse_details(s: std::borrow::Cow<'_, str>) -> Result<Details, Error> {
    let mut lines = s.lines();
    let first = lines.next().ok_or(UploadError::UnsupportedFormat)?;

    let mut segments = first.split('|');

    let dimensions = segments
        .next()
        .ok_or(UploadError::UnsupportedFormat)?
        .trim();
    tracing::debug!("dimensions: {}", dimensions);
    let mut dims = dimensions.split(' ');
    let width = dims
        .next()
        .ok_or(UploadError::UnsupportedFormat)?
        .trim()
        .parse()
        .map_err(|_| UploadError::UnsupportedFormat)?;
    let height = dims
        .next()
        .ok_or(UploadError::UnsupportedFormat)?
        .trim()
        .parse()
        .map_err(|_| UploadError::UnsupportedFormat)?;

    let format = segments
        .next()
        .ok_or(UploadError::UnsupportedFormat)?
        .trim();
    tracing::debug!("format: {}", format);

    if !lines.all(|item| item.ends_with(format)) {
        return Err(UploadError::UnsupportedFormat.into());
    }

    let mime_type = match format {
        "MP4" => crate::validate::video_mp4(),
        "GIF" => mime::IMAGE_GIF,
        "PNG" => mime::IMAGE_PNG,
        "JPEG" => mime::IMAGE_JPEG,
        "WEBP" => crate::validate::image_webp(),
        _ => return Err(UploadError::UnsupportedFormat.into()),
    };

    Ok(Details {
        mime_type,
        width,
        height,
    })
}

pub(crate) async fn input_type_bytes(input: Bytes) -> Result<ValidInputType, Error> {
    details_bytes(input).await?.validate_input()
}

#[instrument(name = "Spawning process command", skip(input))]
pub(crate) fn process_image_write_read(
    input: impl AsyncRead + Unpin + 'static,
    args: Vec<String>,
    format: Format,
) -> std::io::Result<impl AsyncRead + Unpin> {
    let command = "magick";
    let convert_args = ["convert", "-"];
    let last_arg = format!("{}:-", format.to_magick_format());

    let process = Process::spawn(
        Command::new(command)
            .args(convert_args)
            .args(args)
            .arg(last_arg),
    )?;

    Ok(process.write_read(input).unwrap())
}

impl Details {
    fn validate_input(&self) -> Result<ValidInputType, Error> {
        if self.width > crate::CONFIG.max_width() || self.height > crate::CONFIG.max_height() {
            return Err(UploadError::Dimensions.into());
        }

        let input_type = match (self.mime_type.type_(), self.mime_type.subtype()) {
            (mime::VIDEO, mime::MP4 | mime::MPEG) => ValidInputType::Mp4,
            (mime::IMAGE, mime::GIF) => ValidInputType::Gif,
            (mime::IMAGE, mime::PNG) => ValidInputType::Png,
            (mime::IMAGE, mime::JPEG) => ValidInputType::Jpeg,
            (mime::IMAGE, subtype) if subtype.as_str() == "webp" => ValidInputType::Webp,
            _ => return Err(UploadError::UnsupportedFormat.into()),
        };

        Ok(input_type)
    }
}
