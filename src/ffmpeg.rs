use crate::{
    config::{AudioCodec, ImageFormat, MediaConfiguration, VideoCodec},
    error::{Error, UploadError},
    magick::{Details, ValidInputType},
    process::Process,
    store::{Store, StoreError},
};
use actix_web::web::Bytes;
use once_cell::sync::OnceCell;
use std::collections::HashSet;
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug)]
pub(crate) struct TranscodeOptions {
    input_format: VideoFormat,
    output: TranscodeOutputOptions,
}

#[derive(Debug)]
enum TranscodeOutputOptions {
    Gif,
    Video {
        video_codec: VideoCodec,
        audio_codec: Option<AudioCodec>,
    },
}

impl TranscodeOptions {
    pub(crate) fn new(
        media: &MediaConfiguration,
        details: &Details,
        input_format: VideoFormat,
    ) -> Self {
        if let VideoFormat::Gif = input_format {
            if details.width <= media.gif.max_width
                && details.height <= media.gif.max_height
                && details.width * details.height <= media.gif.max_area
                && details.frames.unwrap_or(1) <= media.gif.max_frame_count
            {
                return Self {
                    input_format,
                    output: TranscodeOutputOptions::gif(),
                };
            }
        }

        Self {
            input_format,
            output: TranscodeOutputOptions::video(media),
        }
    }

    pub(crate) const fn needs_reencode(&self) -> bool {
        !matches!(
            (self.input_format, &self.output),
            (VideoFormat::Gif, TranscodeOutputOptions::Gif)
        )
    }

    const fn input_file_extension(&self) -> &'static str {
        self.input_format.to_file_extension()
    }

    const fn output_ffmpeg_format(&self) -> &'static str {
        match self.output {
            TranscodeOutputOptions::Gif => "gif",
            TranscodeOutputOptions::Video { video_codec, .. } => {
                video_codec.to_output_format().to_ffmpeg_format()
            }
        }
    }

    const fn output_file_extension(&self) -> &'static str {
        match self.output {
            TranscodeOutputOptions::Gif => ".gif",
            TranscodeOutputOptions::Video { video_codec, .. } => {
                video_codec.to_output_format().to_file_extension()
            }
        }
    }

    const fn supports_alpha(&self) -> bool {
        matches!(
            self.output,
            TranscodeOutputOptions::Gif
                | TranscodeOutputOptions::Video {
                    video_codec: VideoCodec::Vp8 | VideoCodec::Vp9,
                    ..
                }
        )
    }

    fn execute(
        &self,
        input_path: &str,
        output_path: &str,
        alpha: bool,
    ) -> Result<Process, std::io::Error> {
        match self.output {
            TranscodeOutputOptions::Gif => Process::run("ffmpeg", &[
                "-hide_banner",
                "-v",
                "warning",
                "-i",
                input_path,
                "-filter_complex",
                "[0:v] split [a][b]; [a] palettegen=stats_mode=single [p]; [b][p] paletteuse=new=1",
                "-an",
                "-f",
                self.output_ffmpeg_format(),
                output_path
            ]),
            TranscodeOutputOptions::Video {
                video_codec,
                audio_codec: None,
            } => Process::run(
                "ffmpeg",
                &[
                    "-hide_banner",
                    "-v",
                    "warning",
                    "-i",
                    input_path,
                    "-pix_fmt",
                    video_codec.pix_fmt(alpha),
                    "-vf",
                    "scale=trunc(iw/2)*2:trunc(ih/2)*2",
                    "-an",
                    "-c:v",
                    video_codec.to_ffmpeg_codec(),
                    "-f",
                    self.output_ffmpeg_format(),
                    output_path,
                ],
            ),
            TranscodeOutputOptions::Video {
                video_codec,
                audio_codec: Some(audio_codec),
            } => Process::run(
                "ffmpeg",
                &[
                    "-hide_banner",
                    "-v",
                    "warning",
                    "-i",
                    input_path,
                    "-pix_fmt",
                    video_codec.pix_fmt(alpha),
                    "-vf",
                    "scale=trunc(iw/2)*2:trunc(ih/2)*2",
                    "-c:a",
                    audio_codec.to_ffmpeg_codec(),
                    "-c:v",
                    video_codec.to_ffmpeg_codec(),
                    "-f",
                    self.output_ffmpeg_format(),
                    output_path,
                ],
            ),
        }
    }

    pub(crate) const fn output_type(&self) -> ValidInputType {
        match self.output {
            TranscodeOutputOptions::Gif => ValidInputType::Gif,
            TranscodeOutputOptions::Video { video_codec, .. } => {
                ValidInputType::from_video_codec(video_codec)
            }
        }
    }
}

impl TranscodeOutputOptions {
    fn video(media: &MediaConfiguration) -> Self {
        Self::Video {
            video_codec: media.video_codec,
            audio_codec: if media.enable_full_video {
                Some(
                    media
                        .audio_codec
                        .unwrap_or(media.video_codec.to_output_format().default_audio_codec()),
                )
            } else {
                None
            },
        }
    }

    const fn gif() -> Self {
        Self::Gif
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum VideoFormat {
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

#[derive(Clone, Copy, Debug)]
pub(crate) enum FileFormat {
    Image(ImageFormat),
    Video(VideoFormat),
}

impl ValidInputType {
    pub(crate) fn to_file_format(self) -> FileFormat {
        match self {
            Self::Gif => FileFormat::Video(VideoFormat::Gif),
            Self::Mp4 => FileFormat::Video(VideoFormat::Mp4),
            Self::Webm => FileFormat::Video(VideoFormat::Webm),
            Self::Avif => FileFormat::Image(ImageFormat::Avif),
            Self::Jpeg => FileFormat::Image(ImageFormat::Jpeg),
            Self::Jxl => FileFormat::Image(ImageFormat::Jxl),
            Self::Png => FileFormat::Image(ImageFormat::Png),
            Self::Webp => FileFormat::Image(ImageFormat::Webp),
        }
    }
}

impl VideoFormat {
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

    const fn pix_fmt(&self, alpha: bool) -> &'static str {
        match (self, alpha) {
            (VideoCodec::Vp8 | VideoCodec::Vp9, true) => "yuva420p",
            _ => "yuv420p",
        }
    }
}

impl AudioCodec {
    const fn to_ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "libopus",
            Self::Vorbis => "vorbis",
        }
    }
}

const FORMAT_MAPPINGS: &[(&str, VideoFormat)] = &[
    ("gif", VideoFormat::Gif),
    ("mp4", VideoFormat::Mp4),
    ("webm", VideoFormat::Webm),
];

pub(crate) async fn input_type_bytes(
    input: Bytes,
) -> Result<Option<(Details, ValidInputType)>, Error> {
    if let Some(details) = details_bytes(input).await? {
        let input_type = details.validate_input()?;
        return Ok(Some((details, input_type)));
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

async fn alpha_pixel_formats() -> Result<HashSet<String>, Error> {
    let process = Process::run(
        "ffprobe",
        &[
            "-v",
            "0",
            "-show_entries",
            "pixel_format=name:flags=alpha",
            "-of",
            "compact=p=0",
        ],
    )?;

    let mut output = Vec::new();
    process.read().read_to_end(&mut output).await?;
    let output = String::from_utf8_lossy(&output);
    let formats = output
        .split('\n')
        .filter_map(|format| {
            if format.is_empty() {
                return None;
            }

            if !format.ends_with('1') {
                return None;
            }

            Some(
                format
                    .trim_start_matches("name=")
                    .trim_end_matches("|flags:alpha=1")
                    .to_string(),
            )
        })
        .collect();

    Ok(formats)
}

#[tracing::instrument(skip(f))]
async fn details_file<F, Fut>(f: F) -> Result<Option<Details>, Error>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, Error>>,
{
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(StoreError::from)?;

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
    tracing::debug!("OUTPUT: {}", output);

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
            return parse_details_inner(width, height, frames, *v);
        }
    }

    Ok(None)
}

fn parse_details_inner(
    width: &str,
    height: &str,
    frames: &str,
    format: VideoFormat,
) -> Result<Option<Details>, Error> {
    let width = width.parse().map_err(|_| UploadError::UnsupportedFormat)?;
    let height = height.parse().map_err(|_| UploadError::UnsupportedFormat)?;
    let frames = frames.parse().map_err(|_| UploadError::UnsupportedFormat)?;

    // Probably a still image. ffmpeg thinks AVIF is an mp4
    if frames == 1 {
        return Ok(None);
    }

    Ok(Some(Details {
        mime_type: format.to_mime(),
        width,
        height,
        frames: Some(frames),
    }))
}

async fn pixel_format(input_file: &str) -> Result<String, Error> {
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
            input_file,
        ],
    )?;

    let mut output = Vec::new();
    process.read().read_to_end(&mut output).await?;
    Ok(String::from_utf8_lossy(&output).trim().to_string())
}

#[tracing::instrument(skip(input))]
pub(crate) async fn transcode_bytes(
    input: Bytes,
    transcode_options: TranscodeOptions,
) -> Result<impl AsyncRead + Unpin, Error> {
    let input_file = crate::tmp_file::tmp_file(Some(transcode_options.input_file_extension()));
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(StoreError::from)?;

    let output_file = crate::tmp_file::tmp_file(Some(transcode_options.output_file_extension()));
    let output_file_str = output_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&output_file)
        .await
        .map_err(StoreError::from)?;

    let mut tmp_one = crate::file::File::create(&input_file).await?;
    tmp_one.write_from_bytes(input).await?;
    tmp_one.close().await?;

    let alpha = if transcode_options.supports_alpha() {
        static ALPHA_PIXEL_FORMATS: OnceCell<HashSet<String>> = OnceCell::new();

        let format = pixel_format(input_file_str).await?;

        match ALPHA_PIXEL_FORMATS.get() {
            Some(alpha_pixel_formats) => alpha_pixel_formats.contains(&format),
            None => {
                let pixel_formats = alpha_pixel_formats().await?;
                let alpha = pixel_formats.contains(&format);
                let _ = ALPHA_PIXEL_FORMATS.set(pixel_formats);
                alpha
            }
        }
    } else {
        false
    };

    let process = transcode_options.execute(input_file_str, output_file_str, alpha)?;

    process.wait().await?;
    tokio::fs::remove_file(input_file).await?;

    let tmp_two = crate::file::File::open(&output_file).await?;
    let stream = tmp_two
        .read_to_stream(None, None)
        .await
        .map_err(StoreError::from)?;
    let reader = tokio_util::io::StreamReader::new(stream);
    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, output_file);

    Ok(Box::pin(clean_reader))
}

#[tracing::instrument(skip(store))]
pub(crate) async fn thumbnail<S: Store>(
    store: S,
    from: S::Identifier,
    input_format: VideoFormat,
    format: ThumbnailFormat,
) -> Result<impl AsyncRead + Unpin, Error> {
    let input_file = crate::tmp_file::tmp_file(Some(input_format.to_file_extension()));
    let input_file_str = input_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(StoreError::from)?;

    let output_file = crate::tmp_file::tmp_file(Some(format.to_file_extension()));
    let output_file_str = output_file.to_str().ok_or(UploadError::Path)?;
    crate::store::file_store::safe_create_parent(&output_file)
        .await
        .map_err(StoreError::from)?;

    let mut tmp_one = crate::file::File::create(&input_file).await?;
    tmp_one
        .write_from_stream(store.to_stream(&from, None, None).await?)
        .await?;
    tmp_one.close().await?;

    let process = Process::run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "warning",
            "-i",
            input_file_str,
            "-frames:v",
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
    let stream = tmp_two
        .read_to_stream(None, None)
        .await
        .map_err(StoreError::from)?;
    let reader = tokio_util::io::StreamReader::new(stream);
    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, output_file);

    Ok(Box::pin(clean_reader))
}
