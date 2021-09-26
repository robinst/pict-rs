use crate::{config::Format, error::Error, ffmpeg::InputFormat, magick::ValidInputType};
use actix_web::web::Bytes;
use tokio::io::AsyncRead;
use tracing::instrument;

pub(crate) fn image_webp() -> mime::Mime {
    "image/webp".parse().unwrap()
}

pub(crate) fn video_mp4() -> mime::Mime {
    "video/mp4".parse().unwrap()
}

struct UnvalidatedBytes {
    bytes: Bytes,
    written: usize,
}

impl UnvalidatedBytes {
    fn new(bytes: Bytes) -> Self {
        UnvalidatedBytes { bytes, written: 0 }
    }

    fn boxed(self) -> Box<dyn AsyncRead + Unpin> {
        Box::new(self)
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
    prescribed_format: Option<Format>,
    validate: bool,
) -> Result<(mime::Mime, Box<dyn AsyncRead + Unpin>), Error> {
    let input_type = crate::magick::input_type_bytes(bytes.clone()).await?;

    if !validate {
        let mime_type = match input_type {
            ValidInputType::Gif => video_mp4(),
            ValidInputType::Mp4 => mime::IMAGE_GIF,
            ValidInputType::Jpeg => mime::IMAGE_JPEG,
            ValidInputType::Png => mime::IMAGE_PNG,
            ValidInputType::Webp => image_webp(),
        };

        return Ok((mime_type, UnvalidatedBytes::new(bytes).boxed()));
    }

    match (prescribed_format, input_type) {
        (_, ValidInputType::Gif) => Ok((
            video_mp4(),
            Box::new(crate::ffmpeg::to_mp4_bytes(bytes, InputFormat::Gif)?)
                as Box<dyn AsyncRead + Unpin>,
        )),
        (_, ValidInputType::Mp4) => Ok((
            video_mp4(),
            Box::new(crate::ffmpeg::to_mp4_bytes(bytes, InputFormat::Mp4)?)
                as Box<dyn AsyncRead + Unpin>,
        )),
        (Some(Format::Jpeg) | None, ValidInputType::Jpeg) => Ok((
            mime::IMAGE_JPEG,
            Box::new(crate::exiftool::clear_metadata_bytes_read(bytes)?)
                as Box<dyn AsyncRead + Unpin>,
        )),
        (Some(Format::Png) | None, ValidInputType::Png) => Ok((
            mime::IMAGE_PNG,
            Box::new(crate::exiftool::clear_metadata_bytes_read(bytes)?)
                as Box<dyn AsyncRead + Unpin>,
        )),
        (Some(Format::Webp) | None, ValidInputType::Webp) => Ok((
            image_webp(),
            Box::new(crate::magick::clear_metadata_bytes_read(bytes)?)
                as Box<dyn AsyncRead + Unpin>,
        )),
        (Some(format), _) => Ok((
            format.to_mime(),
            Box::new(crate::magick::convert_bytes_read(bytes, format)?)
                as Box<dyn AsyncRead + Unpin>,
        )),
    }
}
