use crate::{
    config::Format, error::UploadError, exiv2::ValidInputType, magick::ValidFormat, tmp_file,
};

pub(crate) fn image_webp() -> mime::Mime {
    "image/webp".parse().unwrap()
}

pub(crate) fn video_mp4() -> mime::Mime {
    "video/mp4".parse().unwrap()
}

// import & export image using the image crate
#[tracing::instrument]
pub(crate) async fn validate_image(
    tmpfile: std::path::PathBuf,
    prescribed_format: Option<Format>,
) -> Result<mime::Mime, UploadError> {
    let input_type = crate::exiv2::input_type(&tmpfile).await?;

    match (prescribed_format, input_type) {
        (_, ValidInputType::Gif) | (_, ValidInputType::Mp4) => {
            let newfile = tmp_file();
            crate::safe_create_parent(&newfile).await?;
            crate::ffmpeg::to_mp4(&tmpfile, &newfile).await?;
            actix_fs::rename(newfile, tmpfile).await?;

            Ok(video_mp4())
        }
        (Some(Format::Jpeg), ValidInputType::Jpeg) | (None, ValidInputType::Jpeg) => {
            tracing::debug!("Validating format");
            crate::magick::validate_format(&tmpfile, ValidFormat::Jpeg).await?;
            tracing::debug!("Clearing metadata");
            crate::exiv2::clear_metadata(&tmpfile).await?;
            tracing::debug!("Validated");

            Ok(mime::IMAGE_JPEG)
        }
        (Some(Format::Png), ValidInputType::Png) | (None, ValidInputType::Png) => {
            tracing::debug!("Validating format");
            crate::magick::validate_format(&tmpfile, ValidFormat::Png).await?;
            tracing::debug!("Clearing metadata");
            crate::exiv2::clear_metadata(&tmpfile).await?;
            tracing::debug!("Validated");

            Ok(mime::IMAGE_PNG)
        }
        (Some(Format::Webp), ValidInputType::Webp) | (None, ValidInputType::Webp) => {
            tracing::debug!("Validating format");
            crate::magick::validate_format(&tmpfile, ValidFormat::Webp).await?;
            tracing::debug!("Clearing metadata");
            crate::exiv2::clear_metadata(&tmpfile).await?;
            tracing::debug!("Validated");

            Ok(image_webp())
        }
        (Some(format), _) => {
            let newfile = tmp_file();
            crate::safe_create_parent(&newfile).await?;
            crate::magick::convert_file(&tmpfile, &newfile, format.clone()).await?;

            actix_fs::rename(newfile, tmpfile).await?;

            Ok(format.to_mime())
        }
    }
}
