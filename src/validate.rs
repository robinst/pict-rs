use crate::{
    config::{AudioCodec, ImageFormat, VideoCodec},
    either::Either,
    error::{Error, UploadError},
    ffmpeg::FileFormat,
    magick::ValidInputType,
};
use actix_web::web::Bytes;
use tokio::io::AsyncRead;

struct UnvalidatedBytes {
    bytes: Bytes,
    written: usize,
}

impl UnvalidatedBytes {
    fn new(bytes: Bytes) -> Self {
        UnvalidatedBytes { bytes, written: 0 }
    }
}

impl AsyncRead for UnvalidatedBytes {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let bytes_to_write = (self.bytes.len() - self.written).min(buf.remaining());
        if bytes_to_write > 0 {
            let end = self.written + bytes_to_write;
            buf.put_slice(&self.bytes[self.written..end]);
            self.written = end;
        }
        std::task::Poll::Ready(Ok(()))
    }
}

#[tracing::instrument(skip_all)]
pub(crate) async fn validate_bytes(
    bytes: Bytes,
    prescribed_format: Option<ImageFormat>,
    enable_silent_video: bool,
    enable_full_video: bool,
    video_codec: VideoCodec,
    audio_codec: Option<AudioCodec>,
    validate: bool,
) -> Result<(ValidInputType, impl AsyncRead + Unpin), Error> {
    let input_type =
        if let Some(input_type) = crate::ffmpeg::input_type_bytes(bytes.clone()).await? {
            input_type
        } else {
            crate::magick::input_type_bytes(bytes.clone()).await?
        };

    if !validate {
        return Ok((input_type, Either::left(UnvalidatedBytes::new(bytes))));
    }

    match (input_type.to_file_format(), prescribed_format) {
        (FileFormat::Video(video_format), _) => {
            if !(enable_silent_video || enable_full_video) {
                return Err(UploadError::SilentVideoDisabled.into());
            }
            Ok((
                ValidInputType::from_video_codec(video_codec),
                Either::right(Either::left(Either::left(
                    crate::ffmpeg::trancsocde_bytes(
                        bytes,
                        video_format,
                        enable_full_video,
                        video_codec,
                        audio_codec,
                    )
                    .await?,
                ))),
            ))
        }
        (FileFormat::Image(image_format), Some(format)) if image_format != format => Ok((
            ValidInputType::from_format(format),
            Either::right(Either::left(Either::right(
                crate::magick::convert_bytes_read(bytes, format)?,
            ))),
        )),
        (FileFormat::Image(ImageFormat::Webp), _) => Ok((
            ValidInputType::Webp,
            Either::right(Either::left(Either::right(
                crate::magick::convert_bytes_read(bytes, ImageFormat::Webp)?,
            ))),
        )),
        (FileFormat::Image(image_format), _) => {
            if crate::exiftool::needs_reorienting(bytes.clone()).await? {
                Ok((
                    ValidInputType::from_format(image_format),
                    Either::right(Either::left(Either::right(
                        crate::magick::convert_bytes_read(bytes, image_format)?,
                    ))),
                ))
            } else {
                Ok((
                    ValidInputType::from_format(image_format),
                    Either::right(Either::right(crate::exiftool::clear_metadata_bytes_read(
                        bytes,
                    )?)),
                ))
            }
        }
    }
}
