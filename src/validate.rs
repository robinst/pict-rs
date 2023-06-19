use crate::{
    config::{ImageFormat, MediaConfiguration},
    either::Either,
    error::{Error, UploadError},
    ffmpeg::{FileFormat, TranscodeOptions},
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
    media: &MediaConfiguration,
    validate: bool,
) -> Result<(ValidInputType, impl AsyncRead + Unpin), Error> {
    let (details, input_type) =
        if let Some(tup) = crate::ffmpeg::input_type_bytes(bytes.clone()).await? {
            tup
        } else {
            crate::magick::input_type_bytes(bytes.clone()).await?
        };

    if !validate {
        return Ok((input_type, Either::left(UnvalidatedBytes::new(bytes))));
    }

    match (input_type.to_file_format(), media.format) {
        (FileFormat::Video(video_format), _) => {
            if !(media.enable_silent_video || media.enable_full_video) {
                return Err(UploadError::SilentVideoDisabled.into());
            }
            let transcode_options = TranscodeOptions::new(media, &details, video_format);

            if transcode_options.needs_reencode() {
                Ok((
                    transcode_options.output_type(),
                    Either::right(Either::left(Either::left(
                        crate::ffmpeg::transcode_bytes(bytes, transcode_options).await?,
                    ))),
                ))
            } else {
                Ok((
                    transcode_options.output_type(),
                    Either::right(Either::right(crate::exiftool::clear_metadata_bytes_read(
                        bytes,
                    )?)),
                ))
            }
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
