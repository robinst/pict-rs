use std::{ffi::OsStr, sync::Arc};

use uuid::Uuid;

use crate::{
    bytes_stream::BytesStream,
    ffmpeg::FfMpegError,
    formats::{InputVideoFormat, OutputVideo},
    future::WithPollTimer,
    process::{Process, ProcessRead},
    tmp_file::TmpDir,
};

pub(super) async fn transcode_bytes(
    tmp_dir: &TmpDir,
    input_format: InputVideoFormat,
    output_format: OutputVideo,
    crf: u8,
    timeout: u64,
    bytes: BytesStream,
) -> Result<ProcessRead, FfMpegError> {
    let output_file = tmp_dir.tmp_file(None);
    let output_path = output_file.as_os_str();

    let res = crate::ffmpeg::with_file(tmp_dir, None, |input_file| async move {
        crate::file::write_from_stream(&input_file, bytes.into_io_stream())
            .with_poll_timer("write-from-stream")
            .await
            .map_err(FfMpegError::Write)?;

        transcode_files(
            input_file.as_os_str(),
            input_format,
            output_path,
            output_format,
            crf,
            timeout,
        )
        .with_poll_timer("transcode-files")
        .await?;

        let tmp_file = crate::file::File::open(output_path)
            .await
            .map_err(FfMpegError::OpenFile)?;

        tmp_file
            .read_to_stream(None, None)
            .await
            .map_err(FfMpegError::ReadFile)
    })
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

async fn transcode_files(
    input_path: &OsStr,
    input_format: InputVideoFormat,
    output_path: &OsStr,
    output_format: OutputVideo,
    crf: u8,
    timeout: u64,
) -> Result<(), FfMpegError> {
    let crf = crf.to_string();

    let OutputVideo {
        transcode_video,
        transcode_audio,
        format: output_format,
    } = output_format;

    let mut args = vec![
        "-hide_banner".as_ref(),
        "-v".as_ref(),
        "warning".as_ref(),
        "-f".as_ref(),
        input_format.ffmpeg_format().as_ref(),
        "-i".as_ref(),
        input_path,
    ];

    if transcode_video {
        args.extend([
            "-pix_fmt".as_ref(),
            output_format.pix_fmt().as_ref(),
            "-vf".as_ref(),
            "scale=trunc(iw/2)*2:trunc(ih/2)*2".as_ref(),
            "-c:v".as_ref(),
            output_format.ffmpeg_video_codec().as_ref(),
            "-crf".as_ref(),
            crf.as_ref(),
        ] as [&OsStr; 8]);

        if output_format.is_vp9() {
            args.extend(["-b:v".as_ref(), "0".as_ref()] as [&OsStr; 2]);
        }
    } else {
        args.extend(["-c:v".as_ref(), "copy".as_ref()] as [&OsStr; 2]);
    }

    if transcode_audio {
        if let Some(audio_codec) = output_format.ffmpeg_audio_codec() {
            args.extend(["-c:a".as_ref(), audio_codec.as_ref()] as [&OsStr; 2]);
        } else {
            args.push("-an".as_ref())
        }
    } else {
        args.extend(["-c:a".as_ref(), "copy".as_ref()] as [&OsStr; 2]);
    }

    args.extend([
        "-map_metadata".as_ref(),
        "-1".as_ref(),
        "-map_metadata:g".as_ref(),
        "-1".as_ref(),
        "-map_metadata:s".as_ref(),
        "-1".as_ref(),
        "-map_metadata:c".as_ref(),
        "-1".as_ref(),
        "-map_metadata:p".as_ref(),
        "-1".as_ref(),
        "-f".as_ref(),
        output_format.ffmpeg_format().as_ref(),
        output_path,
    ]);

    Process::run("ffmpeg", &args, &[], timeout)
        .await?
        .wait()
        .await?;

    Ok(())
}
