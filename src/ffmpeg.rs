use crate::{
    error::{Error, UploadError},
    process::Process,
    store::Store,
    magick::ValidInputType,
};
use actix_web::web::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::instrument;

#[derive(Copy, Clone, Debug)]
pub(crate) enum InputFormat {
    Gif,
    Mp4,
    Webm,
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum ThumbnailFormat {
    Jpeg,
    // Webp,
}

impl InputFormat {
    fn to_ext(self) -> &'static str {
        match self {
            InputFormat::Gif => ".gif",
            InputFormat::Mp4 => ".mp4",
            InputFormat::Webm => ".webm",
        }
    }

    pub(crate) fn to_valid_input_type(self) -> ValidInputType {
        match self {
            Self::Gif => ValidInputType::Gif,
            Self::Mp4 => ValidInputType::Mp4,
            Self::Webm => ValidInputType::Webm,
        }
    }
}

impl ThumbnailFormat {
    fn as_codec(self) -> &'static str {
        match self {
            ThumbnailFormat::Jpeg => "mjpeg",
            // ThumbnailFormat::Webp => "webp",
        }
    }

    fn to_ext(self) -> &'static str {
        match self {
            ThumbnailFormat::Jpeg => ".jpeg",
        }
    }

    fn as_format(self) -> &'static str {
        match self {
            ThumbnailFormat::Jpeg => "image2",
            // ThumbnailFormat::Webp => "webp",
        }
    }
}

const FORMAT_MAPPINGS: &[(&str, InputFormat)] = &[
    ("gif", InputFormat::Gif),
    ("mp4", InputFormat::Mp4),
    ("webm", InputFormat::Webm),
];

pub(crate) async fn input_type_bytes(
    input: Bytes,
) -> Result<Option<InputFormat>, Error> {
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file).await?;

    let mut tmp_one = crate::file::File::create(&input_file).await?;
    tmp_one.write_from_bytes(input).await?;
    tmp_one.close().await?;

    let process = Process::run("ffprobe", &[
            "-v",
            "quiet",
            "-show_entries",
            "format=format_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            input_file_str,
        ])?;

    let mut output = Vec::new();
    process.read().read_to_end(&mut output).await?;
    let formats = String::from_utf8_lossy(&output);

    tracing::info!("FORMATS: {}", formats);

    for (k, v) in FORMAT_MAPPINGS {
        if formats.contains(k) {
            return Ok(Some(*v))
        }
    }

    Ok(None)
}

#[tracing::instrument(name = "Convert to Mp4", skip(input))]
pub(crate) async fn to_mp4_bytes(
    input: Bytes,
    input_format: InputFormat,
    permit_audio: bool,
) -> Result<impl AsyncRead + Unpin, Error> {
    let input_file = crate::tmp_file::tmp_file(Some(input_format.to_ext()));
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file).await?;

    let output_file = crate::tmp_file::tmp_file(Some(".mp4"));
    let output_file_str = output_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&output_file).await?;

    let mut tmp_one = crate::file::File::create(&input_file).await?;
    tmp_one.write_from_bytes(input).await?;
    tmp_one.close().await?;

    let process = if permit_audio {
        Process::run("ffmpeg", &[
            "-i",
            input_file_str,
            "-pix_fmt",
            "yuv420p",
            "-vf",
            "scale=trunc(iw/2)*2:trunc(ih/2)*2",
            "-c:a",
            "aac",
            "-c:v",
            "h264",
            "-f",
            "mp4",
            output_file_str,
        ])?
    } else {
        Process::run("ffmpeg", &[
            "-i",
            input_file_str,
            "-pix_fmt",
            "yuv420p",
            "-vf",
            "scale=trunc(iw/2)*2:trunc(ih/2)*2",
            "-an",
            "-c:v",
            "h264",
            "-f",
            "mp4",
            output_file_str,
        ])?
    };

    process.wait().await?;
    tokio::fs::remove_file(input_file).await?;

    let tmp_two = crate::file::File::open(&output_file).await?;
    let stream = tmp_two.read_to_stream(None, None).await?;
    let reader = tokio_util::io::StreamReader::new(stream);
    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, output_file);

    Ok(Box::pin(clean_reader))
}

#[instrument(name = "Create video thumbnail")]
pub(crate) async fn thumbnail<S: Store>(
    store: S,
    from: S::Identifier,
    input_format: InputFormat,
    format: ThumbnailFormat,
) -> Result<impl AsyncRead + Unpin, Error> {
    let input_file = crate::tmp_file::tmp_file(Some(input_format.to_ext()));
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file).await?;

    let output_file = crate::tmp_file::tmp_file(Some(format.to_ext()));
    let output_file_str = output_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&output_file).await?;

    let mut tmp_one = crate::file::File::create(&input_file).await?;
    tmp_one
        .write_from_stream(store.to_stream(&from, None, None).await?)
        .await?;
    tmp_one.close().await?;

    let process = Process::run(
        "ffmpeg",
        &[
            "-i",
            input_file_str,
            "-vframes",
            "1",
            "-codec",
            format.as_codec(),
            "-f",
            format.as_format(),
            output_file_str,
        ],
    )?;

    process.wait().await?;
    tokio::fs::remove_file(input_file).await?;

    let tmp_two = crate::file::File::open(&output_file).await?;
    let stream = tmp_two.read_to_stream(None, None).await?;
    let reader = tokio_util::io::StreamReader::new(stream);
    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, output_file);

    Ok(Box::pin(clean_reader))
}
