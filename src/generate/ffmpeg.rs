use std::sync::Arc;

use uuid::Uuid;

use crate::{
    ffmpeg::FfMpegError,
    formats::InternalVideoFormat,
    process::{Process, ProcessRead},
    state::State,
    store::Store,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum ThumbnailFormat {
    Jpeg,
    Png,
    Webp,
}

impl ThumbnailFormat {
    const fn ffmpeg_codec(self) -> &'static str {
        match self {
            Self::Jpeg => "mjpeg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    pub(super) const fn file_extension(self) -> &'static str {
        match self {
            Self::Jpeg => ".jpeg",
            Self::Png => ".png",
            Self::Webp => ".webp",
        }
    }

    const fn ffmpeg_format(self) -> &'static str {
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

#[tracing::instrument(skip(state))]
pub(super) async fn thumbnail<S: Store>(
    state: &State<S>,
    from: Arc<str>,
    input_format: InternalVideoFormat,
    format: ThumbnailFormat,
) -> Result<ProcessRead, FfMpegError> {
    let output_file = state.tmp_dir.tmp_file(Some(format.file_extension()));

    crate::store::file_store::safe_create_parent(&output_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let output_path = output_file.as_os_str();

    let res = crate::ffmpeg::with_file(
        &state.tmp_dir,
        Some(input_format.file_extension()),
        |input_file| async move {
            let stream = state
                .store
                .to_stream(&from, None, None)
                .await
                .map_err(FfMpegError::Store)?;

            crate::file::write_from_stream(&input_file, stream)
                .await
                .map_err(FfMpegError::Write)?;

            Process::run(
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
                    format.ffmpeg_codec().as_ref(),
                    "-f".as_ref(),
                    format.ffmpeg_format().as_ref(),
                    output_path,
                ],
                &[],
                state.config.media.process_timeout,
            )?
            .wait()
            .await
            .map_err(FfMpegError::Process)?;

            let out_file = crate::file::File::open(output_path)
                .await
                .map_err(FfMpegError::OpenFile)?;
            out_file
                .read_to_stream(None, None)
                .await
                .map_err(FfMpegError::ReadFile)
        },
    )
    .await;

    match res {
        Ok(Ok(stream)) => Ok(ProcessRead::new(
            Box::pin(tokio_util::io::StreamReader::new(stream)),
            Arc::from(String::from("ffmpeg")),
            Uuid::now_v7(),
        )
        .add_extras(output_file)),
        Ok(Err(e)) | Err(e) => {
            output_file.cleanup().await.map_err(FfMpegError::Cleanup)?;
            Err(e)
        }
    }
}
