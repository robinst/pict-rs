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

#[instrument(name = "Validate image", skip(bytes))]
pub(crate) async fn validate_image_bytes(
    bytes: Bytes,
    prescribed_format: Option<Format>,
) -> Result<(mime::Mime, Box<dyn AsyncRead + Unpin>), Error> {
    let input_type = crate::magick::input_type_bytes(bytes.clone()).await?;

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
