#[cfg(test)]
mod tests;

use actix_web::web::Bytes;
use tokio::io::AsyncReadExt;

use crate::{
    discover::DiscoverError,
    formats::{AnimationFormat, ImageFormat, ImageInput, InputFile},
    magick::{MagickError, MAGICK_TEMPORARY_PATH},
    process::Process,
    tmp_file::TmpDir,
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
    tmp_dir: &TmpDir,
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
            let frames = count_avif_frames(tmp_dir, timeout, move |mut file| async move {
                file.write_from_bytes(bytes)
                    .await
                    .map_err(MagickError::Write)?;
                Ok(file)
            })
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

    discover_file(tmp_dir, timeout, move |mut file| async move {
        file.write_from_bytes(bytes)
            .await
            .map_err(MagickError::Write)?;

        Ok(file)
    })
    .await
}

#[tracing::instrument(level = "DEBUG", skip(f))]
async fn count_avif_frames<F, Fut>(tmp_dir: &TmpDir, timeout: u64, f: F) -> Result<u32, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let temporary_path = tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_file = tmp_dir.tmp_file(None);
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let envs = [(MAGICK_TEMPORARY_PATH, temporary_path.as_os_str())];

    let process = Process::run(
        "magick",
        &[
            "convert".as_ref(),
            "-ping".as_ref(),
            input_file.as_os_str(),
            "INFO:".as_ref(),
        ],
        &envs,
        timeout,
    )?;

    let mut output = String::new();
    process
        .read()
        .read_to_string(&mut output)
        .await
        .map_err(MagickError::Read)?;

    drop(input_file);
    drop(temporary_path);

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
async fn discover_file<F, Fut>(
    tmp_dir: &TmpDir,
    timeout: u64,
    f: F,
) -> Result<Discovery, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let temporary_path = tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_file = tmp_dir.tmp_file(None);
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (f)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let envs = [(MAGICK_TEMPORARY_PATH, temporary_path.as_os_str())];

    let process = Process::run(
        "magick",
        &[
            "convert".as_ref(),
            "-ping".as_ref(),
            input_file.as_os_str(),
            "JSON:".as_ref(),
        ],
        &envs,
        timeout,
    )?;

    let mut output = Vec::new();
    process
        .read()
        .read_to_end(&mut output)
        .await
        .map_err(MagickError::Read)?;

    drop(input_file);

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
