use crate::{config::Format, error::UploadError, tmp_file};
use actix_web::web;
use magick_rust::MagickWand;
use rexiv2::{MediaType, Metadata};
use std::path::PathBuf;
use tracing::{debug, error, instrument, warn, Span};

pub(crate) mod transcode;

use self::transcode::{Error as TranscodeError, Target};

pub(crate) trait Op {
    fn op<F, T>(&self, f: F) -> Result<T, UploadError>
    where
        F: Fn(&Self) -> Result<T, &'static str>;

    fn op_mut<F, T>(&mut self, f: F) -> Result<T, UploadError>
    where
        F: Fn(&mut Self) -> Result<T, &'static str>;
}

impl Op for MagickWand {
    fn op<F, T>(&self, f: F) -> Result<T, UploadError>
    where
        F: Fn(&Self) -> Result<T, &'static str>,
    {
        match f(self) {
            Ok(t) => Ok(t),
            Err(e) => {
                if let Ok(e) = self.get_exception() {
                    error!("WandError: {}", e.0);
                    Err(UploadError::Wand(e.0))
                } else {
                    Err(UploadError::Wand(e.to_owned()))
                }
            }
        }
    }

    fn op_mut<F, T>(&mut self, f: F) -> Result<T, UploadError>
    where
        F: Fn(&mut Self) -> Result<T, &'static str>,
    {
        match f(self) {
            Ok(t) => Ok(t),
            Err(e) => {
                if let Ok(e) = self.get_exception() {
                    error!("WandError: {}", e.0);
                    Err(UploadError::Wand(e.0))
                } else {
                    Err(UploadError::Wand(e.to_owned()))
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GifError {
    #[error("{0}")]
    Decode(#[from] TranscodeError),

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub(crate) fn image_webp() -> mime::Mime {
    "image/webp".parse().unwrap()
}

pub(crate) fn video_mp4() -> mime::Mime {
    "video/mp4".parse().unwrap()
}

pub(crate) fn ptos(p: &PathBuf) -> Result<String, UploadError> {
    Ok(p.to_str().ok_or(UploadError::Path)?.to_owned())
}

fn validate_format(file: &str, format: &str) -> Result<(), UploadError> {
    let wand = MagickWand::new();
    debug!("reading");
    wand.op(|w| w.read_image(file))?;

    if wand.op(|w| w.get_image_format())? != format {
        return Err(UploadError::UnsupportedFormat);
    }

    Ok(())
}

fn safe_create_parent(path: &PathBuf) -> Result<(), UploadError> {
    if let Some(path) = path.parent() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

// import & export image using the image crate
#[instrument]
pub(crate) async fn validate_image(
    tmpfile: PathBuf,
    prescribed_format: Option<Format>,
) -> Result<mime::Mime, UploadError> {
    let tmpfile_str = ptos(&tmpfile)?;
    let span = Span::current();

    let content_type = web::block(move || {
        let entered = span.enter();

        let meta = Metadata::new_from_path(&tmpfile)?;

        let content_type = match (prescribed_format, meta.get_media_type()?) {
            (_, MediaType::Gif) => {
                let newfile = tmp_file();
                safe_create_parent(&newfile)?;
                validate_frames(&tmpfile, &newfile)?;

                video_mp4()
            }
            (Some(Format::Jpeg), MediaType::Jpeg) | (None, MediaType::Jpeg) => {
                validate_format(&tmpfile_str, "JPEG")?;

                meta.clear();
                meta.save_to_file(&tmpfile)?;

                mime::IMAGE_JPEG
            }
            (Some(Format::Png), MediaType::Png) | (None, MediaType::Png) => {
                validate_format(&tmpfile_str, "PNG")?;

                meta.clear();
                meta.save_to_file(&tmpfile)?;

                mime::IMAGE_PNG
            }
            (Some(Format::Webp), MediaType::Other(webp)) | (None, MediaType::Other(webp))
                if webp == "image/webp" =>
            {
                let newfile = tmp_file();
                safe_create_parent(&newfile)?;
                let newfile_str = ptos(&newfile)?;
                // clean metadata by writing new webp, since exiv2 doesn't support webp yet
                {
                    let wand = MagickWand::new();

                    debug!("reading");
                    wand.op(|w| w.read_image(&tmpfile_str))?;

                    if wand.op(|w| w.get_image_format())? != "WEBP" {
                        return Err(UploadError::UnsupportedFormat);
                    }

                    if let Err(e) = wand.op(|w| w.write_image(&newfile_str)) {
                        std::fs::remove_file(&newfile_str)?;
                        return Err(e);
                    }
                }

                std::fs::rename(&newfile, &tmpfile)?;

                image_webp()
            }
            (Some(format), _) => {
                let newfile = tmp_file();
                safe_create_parent(&newfile)?;
                let newfile_str = ptos(&newfile)?;
                {
                    let mut wand = MagickWand::new();

                    debug!("reading: {}", tmpfile_str);
                    wand.op(|w| w.read_image(&tmpfile_str))?;

                    wand.op_mut(|w| w.set_image_format(format.to_magick_format()))?;

                    debug!("writing: {}", newfile_str);
                    if let Err(e) = wand.op(|w| w.write_image(&newfile_str)) {
                        std::fs::remove_file(&newfile_str)?;
                        return Err(e);
                    }
                }

                std::fs::rename(&newfile, &tmpfile)?;

                format.to_mime()
            }
            (_, MediaType::Other(mp4)) if mp4 == "video/mp4" || mp4 == "video/quicktime" => {
                let newfile = tmp_file();
                safe_create_parent(&newfile)?;
                validate_frames(&tmpfile, &newfile)?;

                video_mp4()
            }
            (_, media_type) => {
                warn!("Unsupported media type, {}", media_type);
                return Err(UploadError::UnsupportedFormat);
            }
        };

        drop(entered);
        Ok(content_type) as Result<mime::Mime, UploadError>
    })
    .await??;

    Ok(content_type)
}

#[instrument]
fn validate_frames(from: &PathBuf, to: &PathBuf) -> Result<(), GifError> {
    debug!("Transmuting GIF");

    if let Err(e) = self::transcode::transcode(from, to, Target::Mp4) {
        std::fs::remove_file(to)?;
        return Err(e.into());
    }

    std::fs::rename(to, from)?;
    Ok(())
}
