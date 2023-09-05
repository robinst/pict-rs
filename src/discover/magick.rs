#[cfg(test)]
mod tests;

use actix_web::web::Bytes;
use tokio::io::AsyncReadExt;

use crate::{
    discover::DiscoverError,
    formats::{AnimationFormat, ImageFormat, ImageInput, InputFile},
    magick::MagickError,
    process::Process,
};

use super::Discovery;

#[derive(Debug, serde::Deserialize)]
struct MagickDiscovery {
    image: Image,
}

#[derive(Debug, serde::Deserialize)]
struct Image {
    format: String,
    geometry: Geometry,
}

#[derive(Debug, serde::Deserialize)]
struct Geometry {
    width: u16,
    height: u16,
}

pub(super) async fn confirm_bytes(
    discovery: Option<Discovery>,
    timeout: u64,
    bytes: Bytes,
) -> Result<Discovery, MagickError> {
    match discovery {
        Some(Discovery {
            input: InputFile::Animation(AnimationFormat::Avif),
            width,
            height,
            ..
        }) => {
            let frames = count_avif_frames(
                move |mut file| async move {
                    file.write_from_bytes(bytes)
                        .await
                        .map_err(MagickError::Write)?;
                    Ok(file)
                },
                timeout,
            )
            .await?;

            if frames == 1 {
                return Ok(Discovery {
                    input: InputFile::Image(ImageInput {
                        format: ImageFormat::Avif,
                        needs_reorient: false,
                    }),
                    width,
                    height,
                    frames: None,
                });
            }

            return Ok(Discovery {
                input: InputFile::Animation(AnimationFormat::Avif),
                width,
                height,
                frames: Some(frames),
            });
        }
        Some(Discovery {
            input: InputFile::Animation(AnimationFormat::Webp),
            ..
        }) => {
            // continue
        }
        Some(otherwise) => return Ok(otherwise),
        None => {
            // continue
        }
    }

    discover_file(
        move |mut file| async move {
            file.write_from_bytes(bytes)
                .await
                .map_err(MagickError::Write)?;

            Ok(file)
        },
        timeout,
    )
    .await
}

#[tracing::instrument(level = "DEBUG", skip(f))]
async fn count_avif_frames<F, Fut>(f: F, timeout: u64) -> Result<u32, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(MagickError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let process = Process::run(
        "magick",
        &["convert", "-ping", input_file_str, "INFO:"],
        timeout,
    )?;

    let mut output = String::new();
    process
        .read()
        .read_to_string(&mut output)
        .await
        .map_err(MagickError::Read)?;
    tokio::fs::remove_file(input_file_str)
        .await
        .map_err(MagickError::RemoveFile)?;

    if output.is_empty() {
        return Err(MagickError::Empty);
    }

    let lines: u32 = output
        .lines()
        .count()
        .try_into()
        .expect("Reasonable frame count");

    if lines == 0 {
        return Err(MagickError::Empty);
    }

    Ok(lines)
}

#[tracing::instrument(level = "DEBUG", skip(f))]
async fn discover_file<F, Fut>(f: F, timeout: u64) -> Result<Discovery, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(MagickError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let process = Process::run(
        "magick",
        &["convert", "-ping", input_file_str, "JSON:"],
        timeout,
    )?;

    let mut output = Vec::new();
    process
        .read()
        .read_to_end(&mut output)
        .await
        .map_err(MagickError::Read)?;
    tokio::fs::remove_file(input_file_str)
        .await
        .map_err(MagickError::RemoveFile)?;

    if output.is_empty() {
        return Err(MagickError::Empty);
    }

    let output: Vec<MagickDiscovery> =
        serde_json::from_slice(&output).map_err(MagickError::Json)?;

    parse_discovery(output).map_err(MagickError::Discover)
}

fn parse_discovery(output: Vec<MagickDiscovery>) -> Result<Discovery, DiscoverError> {
    let frames = output.len();

    if frames == 0 {
        return Err(DiscoverError::NoFrames);
    }

    let width = output
        .iter()
        .map(
            |MagickDiscovery {
                 image:
                     Image {
                         geometry: Geometry { width, .. },
                         ..
                     },
             }| *width,
        )
        .max()
        .expect("Nonempty vector");

    let height = output
        .iter()
        .map(
            |MagickDiscovery {
                 image:
                     Image {
                         geometry: Geometry { height, .. },
                         ..
                     },
             }| *height,
        )
        .max()
        .expect("Nonempty vector");

    let first_format = &output[0].image.format;

    if output.iter().any(
        |MagickDiscovery {
             image: Image { format, .. },
         }| format != first_format,
    ) {
        return Err(DiscoverError::FormatMismatch);
    }

    let frames: u32 = frames.try_into().expect("Reasonable frame count");

    match first_format.as_str() {
        "AVIF" => {
            if frames > 1 {
                Ok(Discovery {
                    input: InputFile::Animation(AnimationFormat::Avif),
                    width,
                    height,
                    frames: Some(frames),
                })
            } else {
                Ok(Discovery {
                    input: InputFile::Image(ImageInput {
                        format: ImageFormat::Avif,
                        needs_reorient: false,
                    }),
                    width,
                    height,
                    frames: None,
                })
            }
        }
        "APNG" => Ok(Discovery {
            input: InputFile::Animation(AnimationFormat::Apng),
            width,
            height,
            frames: Some(frames),
        }),
        "GIF" => Ok(Discovery {
            input: InputFile::Animation(AnimationFormat::Gif),
            width,
            height,
            frames: Some(frames),
        }),
        "JPEG" => Ok(Discovery {
            input: InputFile::Image(ImageInput {
                format: ImageFormat::Jpeg,
                needs_reorient: false,
            }),
            width,
            height,
            frames: None,
        }),
        "JXL" => Ok(Discovery {
            input: InputFile::Image(ImageInput {
                format: ImageFormat::Jxl,
                needs_reorient: false,
            }),
            width,
            height,
            frames: None,
        }),
        "PNG" => Ok(Discovery {
            input: InputFile::Image(ImageInput {
                format: ImageFormat::Png,
                needs_reorient: false,
            }),
            width,
            height,
            frames: None,
        }),
        "WEBP" => {
            if frames > 1 {
                Ok(Discovery {
                    input: InputFile::Animation(AnimationFormat::Webp),
                    width,
                    height,
                    frames: Some(frames),
                })
            } else {
                Ok(Discovery {
                    input: InputFile::Image(ImageInput {
                        format: ImageFormat::Webp,
                        needs_reorient: false,
                    }),
                    width,
                    height,
                    frames: None,
                })
            }
        }
        otherwise => Err(DiscoverError::UnsupportedFileType(String::from(otherwise))),
    }
}
