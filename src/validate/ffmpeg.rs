use std::ffi::OsStr;

use actix_web::web::Bytes;

use crate::{
    ffmpeg::FfMpegError,
    formats::{InputVideoFormat, OutputVideo},
    process::Process,
    read::BoxRead,
    tmp_file::TmpDir,
};

pub(super) async fn transcode_bytes(
    tmp_dir: &TmpDir,
    input_format: InputVideoFormat,
    output_format: OutputVideo,
    crf: u8,
    timeout: u64,
    bytes: Bytes,
) -> Result<BoxRead<'static>, FfMpegError> {
    let input_file = tmp_dir.tmp_file(None);
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

    let output_file = tmp_dir.tmp_file(None);

    transcode_files(
        input_file.as_os_str(),
        input_format,
        output_file.as_os_str(),
        output_format,
        crf,
        timeout,
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
    let clean_reader = output_file.reader(reader);

    Ok(Box::pin(clean_reader))
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
            &crf.as_ref(),
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
        "-f".as_ref(),
        output_format.ffmpeg_format().as_ref(),
        output_path,
    ]);

    Process::run("ffmpeg", &args, &[], timeout)?.wait().await?;

    Ok(())
}
