use crate::{
    config::{AudioCodec, VideoCodec},
    error::{Error, UploadError},
    magick::{Details, ValidInputType},
    process::Process,
    store::Store,
};
use actix_web::web::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::instrument;

#[derive(Clone, Copy, Debug)]
pub(crate) enum InputFormat {
    Gif,
    Mp4,
    Webm,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum OutputFormat {
    Mp4,
    Webm,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ThumbnailFormat {
    Jpeg,
    // Webp,
}

impl InputFormat {
    const fn to_file_extension(self) -> &'static str {
        match self {
            Self::Gif => ".gif",
            Self::Mp4 => ".mp4",
            Self::Webm => ".webm",
        }
    }

    fn to_mime(self) -> mime::Mime {
        match self {
            Self::Gif => mime::IMAGE_GIF,
            Self::Mp4 => crate::magick::video_mp4(),
            Self::Webm => crate::magick::video_webm(),
        }
    }
}

impl ThumbnailFormat {
    const fn as_ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Jpeg => "mjpeg",
            // Self::Webp => "webp",
        }
    }

    const fn to_file_extension(self) -> &'static str {
        match self {
            Self::Jpeg => ".jpeg",
            // Self::Webp => ".webp",
        }
    }

    const fn as_ffmpeg_format(self) -> &'static str {
        match self {
            Self::Jpeg => "image2",
            // Self::Webp => "webp",
        }
    }
}

impl OutputFormat {
    const fn to_ffmpeg_format(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
        }
    }

    const fn default_audio_codec(self) -> AudioCodec {
        match self {
            Self::Mp4 => AudioCodec::Aac,
            Self::Webm => AudioCodec::Opus,
        }
    }

    const fn to_file_extension(self) -> &'static str {
        match self {
            Self::Mp4 => ".mp4",
            Self::Webm => ".webm",
        }
    }
}

impl VideoCodec {
    const fn to_output_format(self) -> OutputFormat {
        match self {
            Self::H264 | Self::H265 => OutputFormat::Mp4,
            Self::Av1 | Self::Vp8 | Self::Vp9 => OutputFormat::Webm,
        }
    }

    const fn to_ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Av1 => "av1",
            Self::H264 => "h264",
            Self::H265 => "hevc",
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp9",
        }
    }
}

impl AudioCodec {
    const fn to_ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
        }
    }
}

const FORMAT_MAPPINGS: &[(&str, InputFormat)] = &[
    ("gif", InputFormat::Gif),
    ("mp4", InputFormat::Mp4),
    ("webm", InputFormat::Webm),
];

pub(crate) async fn input_type_bytes(input: Bytes) -> Result<Option<ValidInputType>, Error> {
    if let Some(details) = details_bytes(input).await? {
        return Ok(Some(details.validate_input()?));
    }

    Ok(None)
}

pub(crate) async fn details_store<S: Store>(
    store: &S,
    identifier: &S::Identifier,
) -> Result<Option<Details>, Error> {
    details_file(move |mut tmp_one| async move {
        let stream = store.to_stream(identifier, None, None).await?;
        tmp_one.write_from_stream(stream).await?;
        Ok(tmp_one)
    })
    .await
}

pub(crate) async fn details_bytes(input: Bytes) -> Result<Option<Details>, Error> {
    details_file(move |mut tmp_one| async move {
        tmp_one.write_from_bytes(input).await?;
        Ok(tmp_one)
    })
    .await
}

async fn details_file<F, Fut>(f: F) -> Result<Option<Details>, Error>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, Error>>,
{
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file).await?;

    let tmp_one = crate::file::File::create(&input_file).await?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await?;

    let process = Process::run(
        "ffprobe",
        &[
            "-v",
            "quiet",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=width,height,nb_read_frames:format=format_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            input_file_str,
        ],
    )?;

    let mut output = Vec::new();
    process.read().read_to_end(&mut output).await?;
    let output = String::from_utf8_lossy(&output);
    tokio::fs::remove_file(input_file_str).await?;

    parse_details(output)
}

fn parse_details(output: std::borrow::Cow<'_, str>) -> Result<Option<Details>, Error> {
    tracing::info!("OUTPUT: {}", output);

    let mut lines = output.lines();

    let width = match lines.next() {
        Some(line) => line,
        None => return Ok(None),
    };

    let height = match lines.next() {
        Some(line) => line,
        None => return Ok(None),
    };

    let frames = match lines.next() {
        Some(line) => line,
        None => return Ok(None),
    };

    let formats = match lines.next() {
        Some(line) => line,
        None => return Ok(None),
    };

    for (k, v) in FORMAT_MAPPINGS {
        if formats.contains(k) {
            return Ok(Some(parse_details_inner(width, height, frames, *v)?));
        }
    }

    Ok(None)
}

fn parse_details_inner(
    width: &str,
    height: &str,
    frames: &str,
    format: InputFormat,
) -> Result<Details, Error> {
    let width = width.parse().map_err(|_| UploadError::UnsupportedFormat)?;
    let height = height.parse().map_err(|_| UploadError::UnsupportedFormat)?;
    let frames = frames.parse().map_err(|_| UploadError::UnsupportedFormat)?;

    Ok(Details {
        mime_type: format.to_mime(),
        width,
        height,
        frames: Some(frames),
    })
}

#[tracing::instrument(name = "Transcode video", skip(input))]
pub(crate) async fn trancsocde_bytes(
    input: Bytes,
    input_format: InputFormat,
    permit_audio: bool,
    video_codec: VideoCodec,
    audio_codec: Option<AudioCodec>,
) -> Result<impl AsyncRead + Unpin, Error> {
    let input_file = crate::tmp_file::tmp_file(Some(input_format.to_file_extension()));
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file).await?;

    let output_file =
        crate::tmp_file::tmp_file(Some(video_codec.to_output_format().to_file_extension()));
    let output_file_str = output_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&output_file).await?;

    let mut tmp_one = crate::file::File::create(&input_file).await?;
    tmp_one.write_from_bytes(input).await?;
    tmp_one.close().await?;

    let output_format = video_codec.to_output_format();
    let audio_codec = audio_codec.unwrap_or(output_format.default_audio_codec());

    let process = if permit_audio {
        Process::run(
            "ffmpeg",
            &[
                "-i",
                input_file_str,
                "-pix_fmt",
                "yuv420p",
                "-vf",
                "scale=trunc(iw/2)*2:trunc(ih/2)*2",
                "-c:a",
                audio_codec.to_ffmpeg_codec(),
                "-c:v",
                video_codec.to_ffmpeg_codec(),
                "-f",
                output_format.to_ffmpeg_format(),
                output_file_str,
            ],
        )?
    } else {
        Process::run(
            "ffmpeg",
            &[
                "-i",
                input_file_str,
                "-pix_fmt",
                "yuv420p",
                "-vf",
                "scale=trunc(iw/2)*2:trunc(ih/2)*2",
                "-an",
                "-c:v",
                video_codec.to_ffmpeg_codec(),
                "-f",
                output_format.to_ffmpeg_format(),
                output_file_str,
            ],
        )?
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
    let input_file = crate::tmp_file::tmp_file(Some(input_format.to_file_extension()));
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file).await?;

    let output_file = crate::tmp_file::tmp_file(Some(format.to_file_extension()));
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
            format.as_ffmpeg_codec(),
            "-f",
            format.as_ffmpeg_format(),
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
