use actix_web::web::Bytes;
use tokio::io::AsyncRead;

use crate::{
    ffmpeg::FfMpegError,
    formats::{OutputVideoFormat, VideoFormat},
    process::Process,
};

pub(super) async fn transcode_bytes(
    input_format: VideoFormat,
    output_format: OutputVideoFormat,
    crf: u8,
    bytes: Bytes,
) -> Result<impl AsyncRead + Unpin, FfMpegError> {
    let input_file = crate::tmp_file::tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(FfMpegError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(FfMpegError::CreateDir)?;

    let mut tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(FfMpegError::CreateFile)?;
    tmp_one
        .write_from_bytes(bytes)
        .await
        .map_err(FfMpegError::Write)?;
    tmp_one.close().await.map_err(FfMpegError::CloseFile)?;

    let output_file = crate::tmp_file::tmp_file(None);
    let output_file_str = output_file.to_str().ok_or(FfMpegError::Path)?;

    transcode_files(
        input_file_str,
        input_format,
        output_file_str,
        output_format,
        crf,
    )
    .await?;

    let tmp_two = crate::file::File::open(&output_file)
        .await
        .map_err(FfMpegError::OpenFile)?;
    let stream = tmp_two
        .read_to_stream(None, None)
        .await
        .map_err(FfMpegError::ReadFile)?;
    let reader = tokio_util::io::StreamReader::new(stream);
    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, output_file);

    Ok(Box::pin(clean_reader))
}

async fn transcode_files(
    input_path: &str,
    input_format: VideoFormat,
    output_path: &str,
    output_format: OutputVideoFormat,
    crf: u8,
) -> Result<(), FfMpegError> {
    let mut args = vec![
        "-hide_banner",
        "-v",
        "warning",
        "-f",
        input_format.ffmpeg_format(),
        "-i",
        input_path,
        "-pix_fmt",
        output_format.pix_fmt(),
        "-vf",
        "scale=trunc(iw/2)*2:trunc(ih/2)*2",
    ];

    if let Some(audio_codec) = output_format.ffmpeg_audio_codec() {
        args.extend(["-c:a", audio_codec]);
    } else {
        args.push("-an")
    }

    args.extend(["-c:v", output_format.ffmpeg_video_codec()]);

    if output_format.is_vp9() {
        args.extend(["-b:v", "0"]);
    }

    let crf = crf.to_string();

    args.extend([
        "-crf",
        &crf,
        "-f",
        output_format.ffmpeg_format(),
        output_path,
    ]);

    Process::run("ffmpeg", &args)?.wait().await?;

    Ok(())
}
