#[cfg(test)]
mod tests;

use std::{collections::HashSet, sync::OnceLock};

use crate::{
    ffmpeg::FfMpegError,
    formats::{
        AlphaCodec, AnimationFormat, ImageFormat, ImageInput, InputFile, InputVideoFormat,
        Mp4AudioCodec, Mp4Codec, WebmAlphaCodec, WebmAudioCodec, WebmCodec,
    },
    process::Process,
    state::State,
    tmp_file::TmpDir,
};
use actix_web::web::Bytes;

use super::Discovery;

const MP4: &str = "mp4";

#[derive(Debug, serde::Deserialize)]
struct FfMpegDiscovery {
    streams: FfMpegStreams,
    format: FfMpegFormat,
}

#[derive(Debug, serde::Deserialize)]
#[serde(transparent)]
struct FfMpegStreams {
    streams: Vec<FfMpegStream>,
}

impl FfMpegStreams {
    fn into_parts(self) -> Option<(FfMpegVideoStream, Option<FfMpegAudioStream>)> {
        let mut video = None;
        let mut audio = None;

        for stream in self.streams {
            match stream {
                FfMpegStream::Video(video_stream) if video.is_none() => {
                    video = Some(video_stream);
                }
                FfMpegStream::Audio(audio_stream) if audio.is_none() => {
                    audio = Some(audio_stream);
                }
                FfMpegStream::Video(FfMpegVideoStream { codec_name, .. }) => {
                    tracing::info!("Encountered duplicate video stream {codec_name:?}");
                }
                FfMpegStream::Audio(FfMpegAudioStream { codec_name, .. }) => {
                    tracing::info!("Encountered duplicate audio stream {codec_name:?}");
                }
                FfMpegStream::Unknown { codec_name } => {
                    tracing::info!("Encountered unknown stream {codec_name}");
                }
            }
        }

        video.map(|v| (v, audio))
    }
}

#[derive(Debug, serde::Deserialize)]
enum FfMpegVideoCodec {
    #[serde(rename = "apng")]
    Apng,
    #[serde(rename = "av1")]
    Av1, // still or animated avif, or av1 video
    #[serde(rename = "gif")]
    Gif,
    #[serde(rename = "h264")]
    H264,
    #[serde(rename = "hevc")]
    Hevc, // h265 video
    #[serde(rename = "mjpeg")]
    Mjpeg,
    #[serde(rename = "jpegxl")]
    Jpegxl,
    #[serde(rename = "png")]
    Png,
    #[serde(rename = "vp8")]
    Vp8,
    #[serde(rename = "vp9")]
    Vp9,
    #[serde(rename = "webp")]
    Webp,
}

#[derive(Debug, serde::Deserialize)]
enum FfMpegAudioCodec {
    #[serde(rename = "aac")]
    Aac,
    #[serde(rename = "opus")]
    Opus,
    #[serde(rename = "vorbis")]
    Vorbis,
}

#[derive(Debug)]
struct FrameString {
    frames: u32,
}

impl<'de> serde::Deserialize<'de> for FrameString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let frames = String::deserialize(deserializer)?
            .parse()
            .map_err(|_| D::Error::custom("Invalid frames string"))?;

        Ok(FrameString { frames })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FfMpegAudioStream {
    codec_name: FfMpegAudioCodec,
}

#[derive(Debug, serde::Deserialize)]
struct FfMpegVideoStream {
    codec_name: FfMpegVideoCodec,
    width: u16,
    height: u16,
    pix_fmt: Option<String>,
    nb_read_frames: Option<FrameString>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum FfMpegStream {
    Audio(FfMpegAudioStream),
    Video(FfMpegVideoStream),
    Unknown { codec_name: String },
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

#[tracing::instrument(skip_all)]
pub(super) async fn discover_bytes<S>(
    state: &State<S>,
    bytes: Bytes,
) -> Result<Option<Discovery>, FfMpegError> {
    discover_file(state, move |mut file| {
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

async fn allows_alpha(pixel_format: &str, timeout: u64) -> Result<bool, FfMpegError> {
    static ALPHA_PIXEL_FORMATS: OnceLock<HashSet<String>> = OnceLock::new();

    match ALPHA_PIXEL_FORMATS.get() {
        Some(alpha_pixel_formats) => Ok(alpha_pixel_formats.contains(pixel_format)),
        None => {
            let pixel_formats = alpha_pixel_formats(timeout).await?;
            let alpha = pixel_formats.contains(pixel_format);
            let _ = ALPHA_PIXEL_FORMATS.set(pixel_formats);
            Ok(alpha)
        }
    }
}

#[tracing::instrument(level = "debug", skip_all)]
async fn discover_file<S, F, Fut>(state: &State<S>, f: F) -> Result<Option<Discovery>, FfMpegError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, FfMpegError>>,
{
    let input_file = state.tmp_dir.tmp_file(None);
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(FfMpegError::CreateFile)?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await.map_err(FfMpegError::CloseFile)?;

    let res = Process::run(
        "ffprobe",
        &[
            "-v".as_ref(),
            "quiet".as_ref(),
            "-count_frames".as_ref(),
            "-show_entries".as_ref(),
            "stream=width,height,nb_read_frames,codec_name,pix_fmt:format=format_name".as_ref(),
            "-of".as_ref(),
            "default=noprint_wrappers=1:nokey=1".as_ref(),
            "-print_format".as_ref(),
            "json".as_ref(),
            input_file.as_os_str(),
        ],
        &[],
        state.config.media.process_timeout,
    )?
    .read()
    .into_vec()
    .await;

    input_file.cleanup().await.map_err(FfMpegError::Cleanup)?;

    let output = res?;

    let output: FfMpegDiscovery = serde_json::from_slice(&output).map_err(FfMpegError::Json)?;

    let (discovery, pix_fmt) = parse_discovery(output)?;

    let Some(mut discovery) = discovery else {
        return Ok(None);
    };

    if let Some(pixel_format) = pix_fmt {
        if let InputFile::Video(InputVideoFormat::Webm {
            video_codec: WebmCodec::Alpha(AlphaCodec { alpha, .. }),
            ..
        }) = &mut discovery.input
        {
            *alpha = allows_alpha(&pixel_format, state.config.media.process_timeout).await?;
        }
    }

    Ok(Some(discovery))
}

#[tracing::instrument(level = "debug", skip_all)]
async fn alpha_pixel_formats(timeout: u64) -> Result<HashSet<String>, FfMpegError> {
    let output = Process::run(
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
        &[],
        timeout,
    )?
    .read()
    .into_vec()
    .await?;

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

fn is_mp4(format_name: &str) -> bool {
    format_name.contains(MP4)
}

fn mp4_audio_codec(stream: Option<FfMpegAudioStream>) -> Option<Mp4AudioCodec> {
    match stream {
        Some(FfMpegAudioStream {
            codec_name: FfMpegAudioCodec::Aac,
        }) => Some(Mp4AudioCodec::Aac),
        _ => None,
    }
}

fn webm_audio_codec(stream: Option<FfMpegAudioStream>) -> Option<WebmAudioCodec> {
    match stream {
        Some(FfMpegAudioStream {
            codec_name: FfMpegAudioCodec::Opus,
        }) => Some(WebmAudioCodec::Opus),
        Some(FfMpegAudioStream {
            codec_name: FfMpegAudioCodec::Vorbis,
        }) => Some(WebmAudioCodec::Vorbis),
        _ => None,
    }
}

fn parse_discovery(
    discovery: FfMpegDiscovery,
) -> Result<(Option<Discovery>, Option<String>), FfMpegError> {
    let FfMpegDiscovery {
        streams,
        format: FfMpegFormat { format_name },
    } = discovery;

    let Some((video_stream, audio_stream)) = streams.into_parts() else {
        tracing::info!("No matching format mapping for {format_name}");
        return Ok((None, None));
    };

    let input = match video_stream.codec_name {
        FfMpegVideoCodec::Av1
            if video_stream
                .nb_read_frames
                .as_ref()
                .is_some_and(|count| count.frames == 1) =>
        {
            // Might be AVIF, ffmpeg incorrectly detects AVIF as single-framed av1 even when
            // animated

            return Ok((
                Some(Discovery {
                    input: InputFile::Animation(AnimationFormat::Avif),
                    width: video_stream.width,
                    height: video_stream.height,
                    frames: None,
                }),
                None,
            ));
        }
        FfMpegVideoCodec::Webp
            if video_stream.height == 0
                || video_stream.width == 0
                || video_stream.nb_read_frames.is_none() =>
        {
            // Might be Animated Webp, ffmpeg incorrectly detects animated webp as having no frames
            // and 0 dimensions

            return Ok((
                Some(Discovery {
                    input: InputFile::Animation(AnimationFormat::Webp),
                    width: video_stream.width,
                    height: video_stream.height,
                    frames: None,
                }),
                None,
            ));
        }
        FfMpegVideoCodec::Av1 if is_mp4(&format_name) => InputFile::Video(InputVideoFormat::Mp4 {
            video_codec: Mp4Codec::Av1,
            audio_codec: mp4_audio_codec(audio_stream),
        }),
        FfMpegVideoCodec::Av1 => InputFile::Video(InputVideoFormat::Webm {
            video_codec: WebmCodec::Av1,
            audio_codec: webm_audio_codec(audio_stream),
        }),
        FfMpegVideoCodec::Apng => InputFile::Animation(AnimationFormat::Apng),
        FfMpegVideoCodec::Gif => InputFile::Animation(AnimationFormat::Gif),
        FfMpegVideoCodec::H264 => InputFile::Video(InputVideoFormat::Mp4 {
            video_codec: Mp4Codec::H264,
            audio_codec: mp4_audio_codec(audio_stream),
        }),
        FfMpegVideoCodec::Hevc => InputFile::Video(InputVideoFormat::Mp4 {
            video_codec: Mp4Codec::H265,
            audio_codec: mp4_audio_codec(audio_stream),
        }),
        FfMpegVideoCodec::Png => InputFile::Image(ImageInput {
            format: ImageFormat::Png,
            needs_reorient: false,
        }),
        FfMpegVideoCodec::Mjpeg => InputFile::Image(ImageInput {
            format: ImageFormat::Jpeg,
            needs_reorient: false,
        }),
        FfMpegVideoCodec::Jpegxl => InputFile::Image(ImageInput {
            format: ImageFormat::Jxl,
            needs_reorient: false,
        }),
        FfMpegVideoCodec::Vp8 => InputFile::Video(InputVideoFormat::Webm {
            video_codec: WebmCodec::Alpha(AlphaCodec {
                alpha: false,
                codec: WebmAlphaCodec::Vp8,
            }),
            audio_codec: webm_audio_codec(audio_stream),
        }),
        FfMpegVideoCodec::Vp9 => InputFile::Video(InputVideoFormat::Webm {
            video_codec: WebmCodec::Alpha(AlphaCodec {
                alpha: false,
                codec: WebmAlphaCodec::Vp9,
            }),
            audio_codec: webm_audio_codec(audio_stream),
        }),
        FfMpegVideoCodec::Webp => InputFile::Image(ImageInput {
            format: ImageFormat::Webp,
            needs_reorient: false,
        }),
    };

    Ok((
        Some(Discovery {
            input,
            width: video_stream.width,
            height: video_stream.height,
            frames: video_stream.nb_read_frames.and_then(|f| {
                if f.frames <= 1 {
                    None
                } else {
                    Some(f.frames)
                }
            }),
        }),
        video_stream.pix_fmt,
    ))
}
