use std::ffi::OsStr;

use actix_web::web::Bytes;

use crate::{
    formats::ProcessableFormat,
    magick::{MagickError, PolicyDir, MAGICK_CONFIGURE_PATH, MAGICK_TEMPORARY_PATH},
    process::{Process, ProcessRead},
    stream::LocalBoxStream,
    tmp_file::TmpDir,
};

async fn thumbnail_animation<F, Fut>(
    tmp_dir: &TmpDir,
    policy_dir: &PolicyDir,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
    write_file: F,
) -> Result<ProcessRead, MagickError>
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
    let tmp_one = (write_file)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let input_arg = [
        input_format.magick_format().as_ref(),
        input_file.as_os_str(),
    ]
    .join(":".as_ref());
    let output_arg = format!("{}:-", format.magick_format());
    let quality = quality.map(|q| q.to_string());

    let len = 3 + if format.coalesce() { 1 } else { 0 } + if quality.is_some() { 1 } else { 0 };

    let mut args: Vec<&OsStr> = Vec::with_capacity(len);
    args.push("convert".as_ref());
    args.push(&input_arg);
    if format.coalesce() {
        args.push("-coalesce".as_ref());
    }
    if let Some(quality) = &quality {
        args.extend(["-quality".as_ref(), quality.as_ref()] as [&OsStr; 2]);
    }
    args.push(output_arg.as_ref());

    let envs = [
        (MAGICK_TEMPORARY_PATH, temporary_path.as_os_str()),
        (MAGICK_CONFIGURE_PATH, policy_dir.as_os_str()),
    ];

    let reader = Process::run("magick", &args, &envs, timeout)?
        .read()
        .add_extras(input_file)
        .add_extras(temporary_path);

    Ok(reader)
}

pub(super) async fn thumbnail(
    tmp_dir: &TmpDir,
    policy_dir: &PolicyDir,
    stream: LocalBoxStream<'static, std::io::Result<Bytes>>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
) -> Result<ProcessRead, MagickError> {
    thumbnail_animation(
        tmp_dir,
        policy_dir,
        input_format,
        format,
        quality,
        timeout,
        |mut tmp_file| async move {
            tmp_file
                .write_from_stream(stream)
                .await
                .map_err(MagickError::Write)?;
            Ok(tmp_file)
        },
    )
    .await
}
