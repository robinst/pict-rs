use crate::{config::Format, error::UploadError, ffmpeg::InputFormat, magick::ValidInputType};

pub(crate) fn image_webp() -> mime::Mime {
    "image/webp".parse().unwrap()
}

pub(crate) fn video_mp4() -> mime::Mime {
    "video/mp4".parse().unwrap()
}

pub(crate) async fn validate_image_stream<S>(
    stream: S,
    prescribed_format: Option<Format>,
) -> Result<
    (
        mime::Mime,
        futures::stream::LocalBoxStream<'static, Result<actix_web::web::Bytes, UploadError>>,
    ),
    UploadError,
>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, UploadError>> + Unpin + 'static,
{
    let (base_stream, copied_stream) = crate::stream::try_duplicate(stream, 1024);

    let input_type =
        crate::magick::input_type_stream::<_, UploadError, UploadError>(Box::pin(base_stream))
            .await?;

    match (prescribed_format, input_type) {
        (_, ValidInputType::Gif) => Ok((
            video_mp4(),
            crate::ffmpeg::to_mp4_stream(copied_stream, InputFormat::Gif)?,
        )),
        (_, ValidInputType::Mp4) => Ok((
            video_mp4(),
            crate::ffmpeg::to_mp4_stream(copied_stream, InputFormat::Mp4)?,
        )),
        (Some(Format::Jpeg) | None, ValidInputType::Jpeg) => Ok((
            mime::IMAGE_JPEG,
            crate::exiftool::clear_metadata_stream(copied_stream)?,
        )),
        (Some(Format::Png) | None, ValidInputType::Png) => Ok((
            mime::IMAGE_PNG,
            crate::exiftool::clear_metadata_stream(copied_stream)?,
        )),
        (Some(Format::Webp) | None, ValidInputType::Webp) => Ok((
            image_webp(),
            crate::magick::clear_metadata_stream(copied_stream)?,
        )),
        (Some(format), _) => Ok((
            format.to_mime(),
            crate::magick::convert_stream(copied_stream, format)?,
        )),
    }
}
