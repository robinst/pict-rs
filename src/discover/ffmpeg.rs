use std::{collections::HashSet, sync::OnceLock};

use crate::{
    ffmpeg::FfMpegError,
    formats::{
        AnimationFormat, AnimationInput, ImageFormat, ImageInput, InputFile, InternalFormat,
        InternalVideoFormat, VideoFormat,
    },
    process::Process,
};
use actix_web::web::Bytes;
use futures_util::Stream;
use tokio::io::AsyncReadExt;

use super::{Discovery, DiscoveryLite};

const FFMPEG_FORMAT_MAPPINGS: &[(&str, InternalFormat)] = &[
    ("apng", InternalFormat::Animation(AnimationFormat::Apng)),
    ("gif", InternalFormat::Animation(AnimationFormat::Gif)),
    ("mp4", InternalFormat::Video(InternalVideoFormat::Mp4)),
    ("png_pipe", InternalFormat::Image(ImageFormat::Png)),
    ("webm", InternalFormat::Video(InternalVideoFormat::Webm)),
    ("webp_pipe", InternalFormat::Image(ImageFormat::Webp)),
];

#[derive(Debug, serde::Deserialize)]
struct FfMpegDiscovery {
    streams: [FfMpegStream; 1],
    format: FfMpegFormat,
}

#[derive(Debug, serde::Deserialize)]
struct FfMpegStream {
    width: u16,
    height: u16,
    nb_read_frames: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct FfMpegFormat {
    format_name: String,
}

#[derive(serde::Deserialize)]
struct PixelFormatOutput {
    pixel_formats: Vec<PixelFormat>,
}

#[derive(serde::Deserialize)]
struct PixelFormat {
    name: String,
    flags: Flags,
}

#[derive(serde::Deserialize)]
struct Flags {
    alpha: usize,
}

pub(super) async fn discover_bytes(bytes: Bytes) -> Result<Option<Discovery>, FfMpegError> {
    discover_file_full(move |mut file| {
        let bytes = bytes.clone();

        async move {
            file.write_from_bytes(bytes)
                .await
                .map_err(FfMpegError::Write)?;
            Ok(file)
        }
    })
    .await
}

pub(super) async fn discover_bytes_lite(
    bytes: Bytes,
) -> Result<Option<DiscoveryLite>, FfMpegError> {
    discover_file_lite(move |mut file| async move {
        file.write_from_bytes(bytes)
            .await
            .map_err(FfMpegError::Write)?;
        Ok(file)
    })
    .await
}

pub(super) async fn discover_stream_lite<S>(stream: S) -> Result<Option<DiscoveryLite>, FfMpegError>
where
    S: Stream<Item = std::io::Result<Bytes>> + Unpin,
{
    discover_file_lite(move |mut file| async move {
        file.write_from_stream(stream)
            .await
            .map_err(FfMpegError::Write)?;
        Ok(file)
    })
    .await
}

async fn discover_file_lite<F, Fut>(f: F) -> Result<Option<DiscoveryLite>, FfMpegError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, FfMpegError>>,
{
    let Some(DiscoveryLite {
        format,
        width,
        height,
        frames,
    }) = discover_file(f)
    .await? else {
        return Ok(None);
    };

    // If we're not confident in our discovery don't return it
    if width == 0 || height == 0 {
        return Ok(None);
    }

    Ok(Some(DiscoveryLite {
        format,
        width,
        height,
        frames,
    }))
}

async fn discover_file_full<F, Fut>(f: F) -> Result<Option<Discovery>, FfMpegError>
where
    F: Fn(crate::file::File) -> Fut + Clone,
    Fut: std::future::Future<Output = Result<crate::file::File, FfMpegError>>,
{
    let Some(DiscoveryLite { format, width, height, frames }) = discover_file(f.clone()).await? else {
        return Ok(None);
    };

    match format {
        InternalFormat::Video(InternalVideoFormat::Webm) => {
            static ALPHA_PIXEL_FORMATS: OnceLock<HashSet<String>> = OnceLock::new();

            let format = pixel_format(f).await?;

            let alpha = match ALPHA_PIXEL_FORMATS.get() {
                Some(alpha_pixel_formats) => alpha_pixel_formats.contains(&format),
                None => {
                    let pixel_formats = alpha_pixel_formats().await?;
                    let alpha = pixel_formats.contains(&format);
                    let _ = ALPHA_PIXEL_FORMATS.set(pixel_formats);
                    alpha
                }
            };

            Ok(Some(Discovery {
                input: InputFile::Video(VideoFormat::Webm { alpha }),
                width,
                height,
                frames,
            }))
        }
        InternalFormat::Video(InternalVideoFormat::Mp4) => Ok(Some(Discovery {
            input: InputFile::Video(VideoFormat::Mp4),
            width,
            height,
            frames,
        })),
        InternalFormat::Animation(format) => Ok(Some(Discovery {
            input: InputFile::Animation(AnimationInput { format }),
            width,
            height,
            frames,
        })),
        InternalFormat::Image(format) => Ok(Some(Discovery {
            input: InputFile::Image(ImageInput {
                format,
                needs_reorient: false,
            }),
            width,
            height,
            frames,
        })),
    }
}

#[tracing::instrument(skip(f))]
async fn discover_file<F, Fut>(f: F) -> Result<Option<DiscoveryLite>, FfMpegError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, FfMpegError>>,
{
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(FfMpegError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(FfMpegError::CreateFile)?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await.map_err(FfMpegError::CloseFile)?;

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
            "-print_format",
            "json",
            input_file_str,
        ],
    )
    .map_err(FfMpegError::Process)?;

    let mut output = Vec::new();
    process
        .read()
        .read_to_end(&mut output)
        .await
        .map_err(FfMpegError::Read)?;
    tokio::fs::remove_file(input_file_str)
        .await
        .map_err(FfMpegError::RemoveFile)?;

    let output: FfMpegDiscovery = serde_json::from_slice(&output).map_err(FfMpegError::Json)?;

    parse_discovery_ffmpeg(output)
}

async fn pixel_format<F, Fut>(f: F) -> Result<String, FfMpegError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, FfMpegError>>,
{
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(FfMpegError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(FfMpegError::CreateFile)?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await.map_err(FfMpegError::CloseFile)?;

    let process = Process::run(
        "ffprobe",
        &[
            "-v",
            "0",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=pix_fmt",
            "-of",
            "compact=p=0:nk=1",
            input_file_str,
        ],
    )
    .map_err(FfMpegError::Process)?;

    let mut output = Vec::new();
    process
        .read()
        .read_to_end(&mut output)
        .await
        .map_err(FfMpegError::Read)?;

    tokio::fs::remove_file(input_file_str)
        .await
        .map_err(FfMpegError::RemoveFile)?;

    Ok(String::from_utf8_lossy(&output).trim().to_string())
}

async fn alpha_pixel_formats() -> Result<HashSet<String>, FfMpegError> {
    let process = Process::run(
        "ffprobe",
        &[
            "-v",
            "0",
            "-show_entries",
            "pixel_format=name:flags=alpha",
            "-of",
            "compact=p=0",
            "-print_format",
            "json",
        ],
    )
    .map_err(FfMpegError::Process)?;

    let mut output = Vec::new();
    process
        .read()
        .read_to_end(&mut output)
        .await
        .map_err(FfMpegError::Read)?;

    let formats: PixelFormatOutput = serde_json::from_slice(&output).map_err(FfMpegError::Json)?;

    Ok(parse_pixel_formats(formats))
}

fn parse_pixel_formats(formats: PixelFormatOutput) -> HashSet<String> {
    formats
        .pixel_formats
        .into_iter()
        .filter_map(|PixelFormat { name, flags }| {
            if flags.alpha == 0 {
                return None;
            }

            Some(name)
        })
        .collect()
}

fn parse_discovery_ffmpeg(
    discovery: FfMpegDiscovery,
) -> Result<Option<DiscoveryLite>, FfMpegError> {
    let FfMpegDiscovery {
        streams:
            [FfMpegStream {
                width,
                height,
                nb_read_frames,
            }],
        format: FfMpegFormat { format_name },
    } = discovery;

    if let Some((name, value)) = FFMPEG_FORMAT_MAPPINGS
        .iter()
        .find(|(name, _)| format_name.contains(name))
    {
        let frames = nb_read_frames.and_then(|frames| frames.parse().ok());

        if *name == "mp4" && frames.map(|nb| nb == 1).unwrap_or(false) {
            // Might be AVIF, ffmpeg incorrectly detects AVIF as single-framed mp4 even when
            // animated

            return Ok(Some(DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Avif),
                width,
                height,
                frames,
            }));
        }

        if *name == "webp" && (frames.is_none() || width == 0 || height == 0) {
            // Might be Animated Webp, ffmpeg incorrectly detects animated webp as having no frames
            // and 0 dimensions

            return Ok(Some(DiscoveryLite {
                format: InternalFormat::Animation(AnimationFormat::Webp),
                width,
                height,
                frames,
            }));
        }

        return Ok(Some(DiscoveryLite {
            format: *value,
            width,
            height,
            frames,
        }));
    }

    tracing::info!("No matching format mapping for {format_name}");

    Ok(None)
}
