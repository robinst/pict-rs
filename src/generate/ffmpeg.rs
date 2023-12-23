use std::sync::Arc;

use uuid::Uuid;

use crate::{
    ffmpeg::FfMpegError,
    formats::InternalVideoFormat,
    process::{Process, ProcessRead},
    store::Store,
    tmp_file::TmpDir,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum ThumbnailFormat {
    Jpeg,
    Png,
    Webp,
}

impl ThumbnailFormat {
    const fn as_ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Jpeg => "mjpeg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    const fn to_file_extension(self) -> &'static str {
        match self {
            Self::Jpeg => ".jpeg",
            Self::Png => ".png",
            Self::Webp => ".webp",
        }
    }

    const fn as_ffmpeg_format(self) -> &'static str {
        match self {
            Self::Jpeg | Self::Png => "image2",
            Self::Webp => "webp",
        }
    }

    pub(crate) fn media_type(self) -> mime::Mime {
        match self {
            Self::Jpeg => mime::IMAGE_JPEG,
            Self::Png => mime::IMAGE_PNG,
            Self::Webp => crate::formats::mimes::image_webp(),
        }
    }
}

#[tracing::instrument(skip(tmp_dir, store, timeout))]
pub(super) async fn thumbnail<S: Store>(
    tmp_dir: &TmpDir,
    store: S,
    from: Arc<str>,
    input_format: InternalVideoFormat,
    format: ThumbnailFormat,
    timeout: u64,
) -> Result<ProcessRead, FfMpegError> {
    let input_file = tmp_dir.tmp_file(Some(input_format.file_extension()));
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let output_file = tmp_dir.tmp_file(Some(format.to_file_extension()));
    crate::store::file_store::safe_create_parent(&output_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let mut tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(FfMpegError::CreateFile)?;
    let stream = store
        .to_stream(&from, None, None)
        .await
        .map_err(FfMpegError::Store)?;
    tmp_one
        .write_from_stream(stream)
        .await
        .map_err(FfMpegError::Write)?;
    tmp_one.close().await.map_err(FfMpegError::CloseFile)?;

    let process = Process::run(
        "ffmpeg",
        &[
            "-hide_banner".as_ref(),
            "-v".as_ref(),
            "warning".as_ref(),
            "-i".as_ref(),
            input_file.as_os_str(),
            "-frames:v".as_ref(),
            "1".as_ref(),
            "-codec".as_ref(),
            format.as_ffmpeg_codec().as_ref(),
            "-f".as_ref(),
            format.as_ffmpeg_format().as_ref(),
            output_file.as_os_str(),
        ],
        &[],
        timeout,
    )?;

    process.wait().await?;
    drop(input_file);

    let tmp_two = crate::file::File::open(&output_file)
        .await
        .map_err(FfMpegError::OpenFile)?;
    let stream = tmp_two
        .read_to_stream(None, None)
        .await
        .map_err(FfMpegError::ReadFile)?;
    let reader = tokio_util::io::StreamReader::new(stream);

    let reader = ProcessRead::new(
        Box::pin(reader),
        Arc::from(String::from("ffmpeg")),
        Uuid::now_v7(),
    )
    .add_extras(output_file);

    Ok(reader)
}
