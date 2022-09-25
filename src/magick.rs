use crate::{
    config::ImageFormat,
    error::{Error, UploadError},
    process::Process,
    repo::Alias,
    store::Store,
};
use actix_web::web::Bytes;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
use tracing::instrument;

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

fn image_webp() -> mime::Mime {
    "image/webp".parse().unwrap()
}

fn video_mp4() -> mime::Mime {
    "video/mp4".parse().unwrap()
}

fn video_webm() -> mime::Mime {
    "video/webm".parse().unwrap()
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum ValidInputType {
    Mp4,
    Webm,
    Gif,
    Png,
    Jpeg,
    Webp,
}

impl ValidInputType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Webm => "WEBM",
            Self::Gif => "GIF",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WEBP",
        }
    }

    pub(crate) fn as_ext(self) -> &'static str {
        match self {
            Self::Mp4 => ".mp4",
            Self::Webm => ".webm",
            Self::Gif => ".gif",
            Self::Png => ".png",
            Self::Jpeg => ".jpeg",
            Self::Webp => ".webp",
        }
    }

    fn video_hint(self) -> Option<&'static str> {
        match self {
            Self::Mp4 => Some(".mp4"),
            Self::Webm => Some(".webm"),
            Self::Gif => Some(".gif"),
            _ => None,
        }
    }

    pub(crate) fn from_format(format: ImageFormat) -> Self {
        match format {
            ImageFormat::Jpeg => ValidInputType::Jpeg,
            ImageFormat::Png => ValidInputType::Png,
            ImageFormat::Webp => ValidInputType::Webp,
        }
    }

    pub(crate) fn to_format(self) -> Option<ImageFormat> {
        match self {
            Self::Jpeg => Some(ImageFormat::Jpeg),
            Self::Png => Some(ImageFormat::Png),
            Self::Webp => Some(ImageFormat::Webp),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Details {
    pub(crate) mime_type: mime::Mime,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) frames: Option<usize>,
}

#[tracing::instrument(name = "Clear Metadata", skip(input))]
pub(crate) fn clear_metadata_bytes_read(input: Bytes) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::run("magick", &["convert", "-", "-strip", "-"])?;

    Ok(process.bytes_read(input))
}

#[tracing::instrument(name = "Convert", skip(input))]
pub(crate) fn convert_bytes_read(
    input: Bytes,
    format: ImageFormat,
) -> std::io::Result<impl AsyncRead + Unpin> {
    let process = Process::run(
        "magick",
        &[
            "convert",
            "-",
            "-strip",
            format!("{}:-", format.as_magick_format()).as_str(),
        ],
    )?;

    Ok(process.bytes_read(input))
}

#[instrument(name = "Getting details from input bytes", skip(input))]
pub(crate) async fn details_bytes(
    input: Bytes,
    hint: Option<ValidInputType>,
) -> Result<Details, Error> {
    if let Some(hint) = hint.and_then(|hint| hint.video_hint()) {
        let input_file = crate::tmp_file::tmp_file(Some(hint));
        let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
        crate::store::file_store::safe_create_parent(&input_file).await?;

        let mut tmp_one = crate::file::File::create(&input_file).await?;
        tmp_one.write_from_bytes(input).await?;
        tmp_one.close().await?;

        return details_file(input_file_str).await;
    }

    let last_arg = if let Some(expected_format) = hint {
        format!("{}:-", expected_format.as_str())
    } else {
        "-".to_owned()
    };

    let process = Process::run(
        "magick",
        &["identify", "-ping", "-format", "%w %h | %m\n", &last_arg],
    )?;

    let mut reader = process.bytes_read(input);

    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    let s = String::from_utf8_lossy(&bytes);

    parse_details(s)
}

#[tracing::instrument(skip(store))]
pub(crate) async fn details_store<S: Store + 'static>(
    store: S,
    identifier: S::Identifier,
    hint: Option<ValidInputType>,
) -> Result<Details, Error> {
    if let Some(hint) = hint.and_then(|hint| hint.video_hint()) {
        let input_file = crate::tmp_file::tmp_file(Some(hint));
        let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
        crate::store::file_store::safe_create_parent(&input_file).await?;

        let mut tmp_one = crate::file::File::create(&input_file).await?;
        tmp_one
            .write_from_stream(store.to_stream(&identifier, None, None).await?)
            .await?;
        tmp_one.close().await?;

        return details_file(input_file_str).await;
    }

    let last_arg = if let Some(expected_format) = hint {
        format!("{}:-", expected_format.as_str())
    } else {
        "-".to_owned()
    };

    let process = Process::run(
        "magick",
        &["identify", "-ping", "-format", "%w %h | %m\n", &last_arg],
    )?;

    let mut reader = process.store_read(store, identifier);

    let mut output = Vec::new();
    reader.read_to_end(&mut output).await?;

    let s = String::from_utf8_lossy(&output);

    parse_details(s)
}

#[tracing::instrument]
pub(crate) async fn details_file(path_str: &str) -> Result<Details, Error> {
    let process = Process::run(
        "magick",
        &["identify", "-ping", "-format", "%w %h | %m\n", path_str],
    )?;

    let mut reader = process.read();

    let mut output = Vec::new();
    reader.read_to_end(&mut output).await?;
    tokio::fs::remove_file(path_str).await?;

    let s = String::from_utf8_lossy(&output);

    parse_details(s)
}

fn parse_details(s: std::borrow::Cow<'_, str>) -> Result<Details, Error> {
    let frames = s.lines().count();

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
        "MP4" => video_mp4(),
        "WEBM" => video_webm(),
        "GIF" => mime::IMAGE_GIF,
        "PNG" => mime::IMAGE_PNG,
        "JPEG" => mime::IMAGE_JPEG,
        "WEBP" => image_webp(),
        _ => return Err(UploadError::UnsupportedFormat.into()),
    };

    Ok(Details {
        mime_type,
        width,
        height,
        frames: if frames > 1 {
            Some(frames)
        } else {
            None
        },
    })
}

#[instrument(name = "Getting input type from bytes", skip(input))]
pub(crate) async fn input_type_bytes(input: Bytes) -> Result<ValidInputType, Error> {
    details_bytes(input, None).await?.validate_input()
}

fn process_image(args: Vec<String>, format: ImageFormat) -> std::io::Result<Process> {
    let command = "magick";
    let convert_args = ["convert", "-"];
    let last_arg = format!("{}:-", format.as_magick_format());

    Process::spawn(
        Command::new(command)
            .args(convert_args)
            .args(args)
            .arg(last_arg),
    )
}

#[instrument(name = "Spawning process command")]
pub(crate) fn process_image_store_read<S: Store + 'static>(
    store: S,
    identifier: S::Identifier,
    args: Vec<String>,
    format: ImageFormat,
) -> std::io::Result<impl AsyncRead + Unpin> {
    Ok(process_image(args, format)?.store_read(store, identifier))
}

#[instrument(name = "Spawning process command", skip(async_read))]
pub(crate) fn process_image_async_read<A: AsyncRead + Unpin + 'static>(
    async_read: A,
    args: Vec<String>,
    format: ImageFormat,
) -> std::io::Result<impl AsyncRead + Unpin> {
    Ok(process_image(args, format)?.pipe_async_read(async_read))
}

impl Details {
    #[instrument(name = "Validating input type")]
    fn validate_input(&self) -> Result<ValidInputType, Error> {
        if self.width > crate::CONFIG.media.max_width
            || self.height > crate::CONFIG.media.max_height
            || self.width * self.height > crate::CONFIG.media.max_area
        {
            return Err(UploadError::Dimensions.into());
        }

        if let Some(frames) = self.frames {
            if frames > crate::CONFIG.media.max_frame_count {
                return Err(UploadError::Frames.into());
            }
        }

        let input_type = match (self.mime_type.type_(), self.mime_type.subtype()) {
            (mime::VIDEO, mime::MP4 | mime::MPEG) => ValidInputType::Mp4,
            (mime::VIDEO, subtype) if subtype.as_str() == "webm" => ValidInputType::Webm,
            (mime::IMAGE, mime::GIF) => ValidInputType::Gif,
            (mime::IMAGE, mime::PNG) => ValidInputType::Png,
            (mime::IMAGE, mime::JPEG) => ValidInputType::Jpeg,
            (mime::IMAGE, subtype) if subtype.as_str() == "webp" => ValidInputType::Webp,
            _ => return Err(UploadError::UnsupportedFormat.into()),
        };

        Ok(input_type)
    }
}
