#[cfg(test)]
mod tests;

use crate::{
    config::{ImageFormat, VideoCodec},
    formats::ProcessableFormat,
    process::{Process, ProcessError},
    repo::Alias,
    store::Store,
};
use actix_web::web::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("Error in imagemagick process")]
    Process(#[source] ProcessError),

    #[error("Error parsing details")]
    ParseDetails(#[source] ParseDetailsError),

    #[error("Media details are invalid")]
    ValidateDetails(#[source] ValidateDetailsError),

    #[error("Invalid output format")]
    Json(#[source] serde_json::Error),

    #[error("Error reading bytes")]
    Read(#[source] std::io::Error),

    #[error("Error writing bytes")]
    Write(#[source] std::io::Error),

    #[error("Error creating file")]
    CreateFile(#[source] std::io::Error),

    #[error("Error creating directory")]
    CreateDir(#[source] crate::store::file_store::FileError),

    #[error("Error reading file")]
    Store(#[source] crate::store::StoreError),

    #[error("Error closing file")]
    CloseFile(#[source] std::io::Error),

    #[error("Error removing file")]
    RemoveFile(#[source] std::io::Error),

    #[error("Invalid file path")]
    Path,
}

impl MagickError {
    pub(crate) fn is_client_error(&self) -> bool {
        // Failing validation or imagemagick bailing probably means bad input
        matches!(self, Self::ValidateDetails(_))
            || matches!(self, Self::Process(ProcessError::Status(_)))
    }

    pub(crate) fn is_not_found(&self) -> bool {
        if let Self::Store(e) = self {
            return e.is_not_found();
        }

        false
    }
}

pub(crate) fn details_hint(alias: &Alias) -> Option<ValidInputType> {
    let ext = alias.extension()?;
    if ext.ends_with(".mp4") {
        Some(ValidInputType::Mp4)
    } else if ext.ends_with(".webm") {
        Some(ValidInputType::Webm)
    } else {
        None
    }
}

fn image_avif() -> mime::Mime {
    "image/avif".parse().unwrap()
}

fn image_jxl() -> mime::Mime {
    "image/jxl".parse().unwrap()
}

fn image_webp() -> mime::Mime {
    "image/webp".parse().unwrap()
}

pub(crate) fn video_mp4() -> mime::Mime {
    "video/mp4".parse().unwrap()
}

pub(crate) fn video_webm() -> mime::Mime {
    "video/webm".parse().unwrap()
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum ValidInputType {
    Mp4,
    Webm,
    Gif,
    Avif,
    Jpeg,
    Jxl,
    Png,
    Webp,
}

impl ValidInputType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Webm => "WEBM",
            Self::Gif => "GIF",
            Self::Avif => "AVIF",
            Self::Jpeg => "JPEG",
            Self::Jxl => "JXL",
            Self::Png => "PNG",
            Self::Webp => "WEBP",
        }
    }

    pub(crate) const fn as_ext(self) -> &'static str {
        match self {
            Self::Mp4 => ".mp4",
            Self::Webm => ".webm",
            Self::Gif => ".gif",
            Self::Avif => ".avif",
            Self::Jpeg => ".jpeg",
            Self::Jxl => ".jxl",
            Self::Png => ".png",
            Self::Webp => ".webp",
        }
    }

    pub(crate) const fn is_video(self) -> bool {
        matches!(self, Self::Mp4 | Self::Webm | Self::Gif)
    }

    const fn video_hint(self) -> Option<&'static str> {
        match self {
            Self::Mp4 => Some(".mp4"),
            Self::Webm => Some(".webm"),
            Self::Gif => Some(".gif"),
            _ => None,
        }
    }

    pub(crate) const fn from_video_codec(codec: VideoCodec) -> Self {
        match codec {
            VideoCodec::Av1 | VideoCodec::Vp8 | VideoCodec::Vp9 => Self::Webm,
            VideoCodec::H264 | VideoCodec::H265 => Self::Mp4,
        }
    }

    pub(crate) const fn from_format(format: ImageFormat) -> Self {
        match format {
            ImageFormat::Avif => ValidInputType::Avif,
            ImageFormat::Jpeg => ValidInputType::Jpeg,
            ImageFormat::Jxl => ValidInputType::Jxl,
            ImageFormat::Png => ValidInputType::Png,
            ImageFormat::Webp => ValidInputType::Webp,
        }
    }

    pub(crate) const fn to_format(self) -> Option<ImageFormat> {
        match self {
            Self::Avif => Some(ImageFormat::Avif),
            Self::Jpeg => Some(ImageFormat::Jpeg),
            Self::Jxl => Some(ImageFormat::Jxl),
            Self::Png => Some(ImageFormat::Png),
            Self::Webp => Some(ImageFormat::Webp),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Details {
    pub(crate) mime_type: mime::Mime,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) frames: Option<usize>,
}

#[tracing::instrument(level = "debug", skip(input))]
pub(crate) fn convert_bytes_read(
    input: Bytes,
    format: ImageFormat,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    let process = Process::run(
        "magick",
        &[
            "convert",
            "-",
            "-auto-orient",
            "-strip",
            format!("{}:-", format.as_magick_format()).as_str(),
        ],
    )
    .map_err(MagickError::Process)?;

    Ok(process.bytes_read(input))
}

#[tracing::instrument(skip(input))]
pub(crate) async fn details_bytes(
    input: Bytes,
    hint: Option<ValidInputType>,
) -> Result<Details, MagickError> {
    if let Some(hint) = hint.and_then(|hint| hint.video_hint()) {
        let input_file = crate::tmp_file::tmp_file(Some(hint));
        let input_file_str = input_file.to_str().ok_or(MagickError::Path)?;
        crate::store::file_store::safe_create_parent(&input_file)
            .await
            .map_err(MagickError::CreateDir)?;

        let mut tmp_one = crate::file::File::create(&input_file)
            .await
            .map_err(MagickError::CreateFile)?;
        tmp_one
            .write_from_bytes(input)
            .await
            .map_err(MagickError::Write)?;
        tmp_one.close().await.map_err(MagickError::CloseFile)?;

        return details_file(input_file_str).await;
    }

    let last_arg = if let Some(expected_format) = hint {
        format!("{}:-", expected_format.as_str())
    } else {
        "-".to_owned()
    };

    let process = Process::run("magick", &["convert", "-ping", &last_arg, "JSON:"])
        .map_err(MagickError::Process)?;

    let mut reader = process.bytes_read(input);

    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(MagickError::Read)?;

    let details_output: Vec<DetailsOutput> =
        serde_json::from_slice(&bytes).map_err(MagickError::Json)?;

    parse_details(details_output).map_err(MagickError::ParseDetails)
}

#[derive(Debug, serde::Deserialize)]
struct DetailsOutput {
    image: Image,
}

#[derive(Debug, serde::Deserialize)]
struct Image {
    format: String,
    geometry: Geometry,
}

#[derive(Debug, serde::Deserialize)]
struct Geometry {
    width: usize,
    height: usize,
}

#[tracing::instrument(skip(store))]
pub(crate) async fn details_store<S: Store + 'static>(
    store: S,
    identifier: S::Identifier,
    hint: Option<ValidInputType>,
) -> Result<Details, MagickError> {
    if let Some(hint) = hint.and_then(|hint| hint.video_hint()) {
        let input_file = crate::tmp_file::tmp_file(Some(hint));
        let input_file_str = input_file.to_str().ok_or(MagickError::Path)?;
        crate::store::file_store::safe_create_parent(&input_file)
            .await
            .map_err(MagickError::CreateDir)?;

        let mut tmp_one = crate::file::File::create(&input_file)
            .await
            .map_err(MagickError::CreateFile)?;
        let stream = store
            .to_stream(&identifier, None, None)
            .await
            .map_err(MagickError::Store)?;
        tmp_one
            .write_from_stream(stream)
            .await
            .map_err(MagickError::Write)?;
        tmp_one.close().await.map_err(MagickError::CloseFile)?;

        return details_file(input_file_str).await;
    }

    let last_arg = if let Some(expected_format) = hint {
        format!("{}:-", expected_format.as_str())
    } else {
        "-".to_owned()
    };

    let process = Process::run("magick", &["convert", "-ping", &last_arg, "JSON:"])
        .map_err(MagickError::Process)?;

    let mut reader = process.store_read(store, identifier);

    let mut output = Vec::new();
    reader
        .read_to_end(&mut output)
        .await
        .map_err(MagickError::Read)?;

    let details_output: Vec<DetailsOutput> =
        serde_json::from_slice(&output).map_err(MagickError::Json)?;

    parse_details(details_output).map_err(MagickError::ParseDetails)
}

#[tracing::instrument]
pub(crate) async fn details_file(path_str: &str) -> Result<Details, MagickError> {
    let process = Process::run("magick", &["convert", "-ping", path_str, "JSON:"])
        .map_err(MagickError::Process)?;

    let mut reader = process.read();

    let mut output = Vec::new();
    reader
        .read_to_end(&mut output)
        .await
        .map_err(MagickError::Read)?;
    tokio::fs::remove_file(path_str)
        .await
        .map_err(MagickError::RemoveFile)?;

    let details_output: Vec<DetailsOutput> =
        serde_json::from_slice(&output).map_err(MagickError::Json)?;

    parse_details(details_output).map_err(MagickError::ParseDetails)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ParseDetailsError {
    #[error("No frames present in image")]
    NoFrames,

    #[error("Multiple image formats used in same file")]
    MixedFormats,

    #[error("Format is unsupported: {0}")]
    Unsupported(String),

    #[error("Could not parse frame count from {0}")]
    ParseFrames(String),
}

fn parse_details(details_output: Vec<DetailsOutput>) -> Result<Details, ParseDetailsError> {
    let frames = details_output.len();

    if frames == 0 {
        return Err(ParseDetailsError::NoFrames);
    }

    let width = details_output
        .iter()
        .map(|details| details.image.geometry.width)
        .max()
        .expect("Nonempty vector");
    let height = details_output
        .iter()
        .map(|details| details.image.geometry.height)
        .max()
        .expect("Nonempty vector");

    let format = details_output[0].image.format.as_str();
    tracing::debug!("format: {}", format);

    if !details_output
        .iter()
        .all(|details| details.image.format == format)
    {
        return Err(ParseDetailsError::MixedFormats);
    }

    let mime_type = match format {
        "MP4" => video_mp4(),
        "WEBM" => video_webm(),
        "GIF" => mime::IMAGE_GIF,
        "AVIF" => image_avif(),
        "JPEG" => mime::IMAGE_JPEG,
        "JXL" => image_jxl(),
        "PNG" => mime::IMAGE_PNG,
        "WEBP" => image_webp(),
        e => return Err(ParseDetailsError::Unsupported(String::from(e))),
    };

    Ok(Details {
        mime_type,
        width,
        height,
        frames: if frames > 1 { Some(frames) } else { None },
    })
}

pub(crate) async fn input_type_bytes(
    input: Bytes,
) -> Result<(Details, ValidInputType), MagickError> {
    let details = details_bytes(input, None).await?;
    let input_type = details
        .validate_input()
        .map_err(MagickError::ValidateDetails)?;
    Ok((details, input_type))
}

fn process_image(
    process_args: Vec<String>,
    format: ProcessableFormat,
) -> Result<Process, ProcessError> {
    let command = "magick";
    let convert_args = ["convert", "-"];
    let last_arg = format!("{}:-", format.magick_format());

    let len = if format.coalesce() {
        process_args.len() + 4
    } else {
        process_args.len() + 3
    };

    let mut args = Vec::with_capacity(len);
    args.extend_from_slice(&convert_args[..]);
    args.extend(process_args.iter().map(|s| s.as_str()));
    if format.coalesce() {
        args.push("-coalesce");
    }
    args.push(&last_arg);

    Process::run(command, &args)
}

pub(crate) fn process_image_store_read<S: Store + 'static>(
    store: S,
    identifier: S::Identifier,
    args: Vec<String>,
    format: ProcessableFormat,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    Ok(process_image(args, format)
        .map_err(MagickError::Process)?
        .store_read(store, identifier))
}

pub(crate) fn process_image_async_read<A: AsyncRead + Unpin + 'static>(
    async_read: A,
    args: Vec<String>,
    format: ProcessableFormat,
) -> Result<impl AsyncRead + Unpin, MagickError> {
    Ok(process_image(args, format)
        .map_err(MagickError::Process)?
        .pipe_async_read(async_read))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidateDetailsError {
    #[error("Exceeded maximum dimensions")]
    ExceededDimensions,

    #[error("Exceeded maximum frame count")]
    TooManyFrames,

    #[error("Unsupported media type: {0}")]
    UnsupportedMediaType(String),
}

impl Details {
    #[tracing::instrument(level = "debug", name = "Validating input type")]
    pub(crate) fn validate_input(&self) -> Result<ValidInputType, ValidateDetailsError> {
        if self.width > crate::CONFIG.media.max_width
            || self.height > crate::CONFIG.media.max_height
            || self.width * self.height > crate::CONFIG.media.max_area
        {
            return Err(ValidateDetailsError::ExceededDimensions);
        }

        if let Some(frames) = self.frames {
            if frames > crate::CONFIG.media.max_frame_count {
                return Err(ValidateDetailsError::TooManyFrames);
            }
        }

        let input_type = match (self.mime_type.type_(), self.mime_type.subtype()) {
            (mime::VIDEO, mime::MP4 | mime::MPEG) => ValidInputType::Mp4,
            (mime::VIDEO, subtype) if subtype.as_str() == "webm" => ValidInputType::Webm,
            (mime::IMAGE, mime::GIF) => ValidInputType::Gif,
            (mime::IMAGE, subtype) if subtype.as_str() == "avif" => ValidInputType::Avif,
            (mime::IMAGE, mime::JPEG) => ValidInputType::Jpeg,
            (mime::IMAGE, subtype) if subtype.as_str() == "jxl" => ValidInputType::Jxl,
            (mime::IMAGE, mime::PNG) => ValidInputType::Png,
            (mime::IMAGE, subtype) if subtype.as_str() == "webp" => ValidInputType::Webp,
            _ => {
                return Err(ValidateDetailsError::UnsupportedMediaType(
                    self.mime_type.to_string(),
                ))
            }
        };

        Ok(input_type)
    }
}
