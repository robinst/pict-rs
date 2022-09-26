use crate::{
    config::ImageFormat,
    either::Either,
    error::{Error, UploadError},
    ffmpeg::InputFormat,
    magick::ValidInputType,
};
use actix_web::web::Bytes;
use tokio::io::AsyncRead;
use tracing::instrument;

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

#[instrument(name = "Validate image", skip(bytes))]
pub(crate) async fn validate_image_bytes(
    bytes: Bytes,
    prescribed_format: Option<ImageFormat>,
    enable_silent_video: bool,
    enable_full_video: bool,
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

    match (prescribed_format, input_type) {
        (_, ValidInputType::Gif) => {
            if !(enable_silent_video || enable_full_video) {
                return Err(UploadError::SilentVideoDisabled.into());
            }
            Ok((
                ValidInputType::Mp4,
                Either::right(Either::left(
                    crate::ffmpeg::to_mp4_bytes(bytes, InputFormat::Gif, false).await?,
                )),
            ))
        }
        (_, ValidInputType::Mp4) => {
            if !(enable_silent_video || enable_full_video) {
                return Err(UploadError::SilentVideoDisabled.into());
            }
            Ok((
                ValidInputType::Mp4,
                Either::right(Either::left(
                    crate::ffmpeg::to_mp4_bytes(bytes, InputFormat::Mp4, enable_full_video).await?,
                )),
            ))
        }
        (_, ValidInputType::Webm) => {
            if !(enable_silent_video || enable_full_video) {
                return Err(UploadError::SilentVideoDisabled.into());
            }
            Ok((
                ValidInputType::Mp4,
                Either::right(Either::left(
                    crate::ffmpeg::to_mp4_bytes(bytes, InputFormat::Webm, enable_full_video)
                        .await?,
                )),
            ))
        }
        (Some(ImageFormat::Jpeg) | None, ValidInputType::Jpeg) => Ok((
            ValidInputType::Jpeg,
            Either::right(Either::right(Either::left(
                crate::exiftool::clear_metadata_bytes_read(bytes)?,
            ))),
        )),
        (Some(ImageFormat::Png) | None, ValidInputType::Png) => Ok((
            ValidInputType::Png,
            Either::right(Either::right(Either::left(
                crate::exiftool::clear_metadata_bytes_read(bytes)?,
            ))),
        )),
        (Some(ImageFormat::Webp) | None, ValidInputType::Webp) => Ok((
            ValidInputType::Webp,
            Either::right(Either::right(Either::right(Either::left(
                crate::magick::clear_metadata_bytes_read(bytes)?,
            )))),
        )),
        (Some(format), _) => Ok((
            ValidInputType::from_format(format),
            Either::right(Either::right(Either::right(Either::right(
                crate::magick::convert_bytes_read(bytes, format)?,
            )))),
        )),
    }
}
