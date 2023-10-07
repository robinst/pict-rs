use std::sync::Arc;

use crate::{
    formats::ProcessableFormat, magick::MagickError, process::Process, read::BoxRead, store::Store,
    tmp_file::TmpDir,
};

async fn thumbnail_animation<F, Fut>(
    tmp_dir: &TmpDir,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
    write_file: F,
) -> Result<BoxRead<'static>, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let input_file = tmp_dir.tmp_file(None);
    let input_file_str = input_file.to_str().ok_or(MagickError::Path)?;
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (write_file)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let input_arg = format!("{}:{input_file_str}[0]", input_format.magick_format());
    let output_arg = format!("{}:-", format.magick_format());
    let quality = quality.map(|q| q.to_string());

    let len = 3 + if format.coalesce() { 1 } else { 0 } + if quality.is_some() { 1 } else { 0 };

    let mut args: Vec<&str> = Vec::with_capacity(len);
    args.push("convert");
    args.push(&input_arg);
    if format.coalesce() {
        args.push("-coalesce");
    }
    if let Some(quality) = &quality {
        args.extend(["-quality", quality]);
    }
    args.push(&output_arg);

    let reader = Process::run("magick", &args, timeout)?.read();

    let clean_reader = crate::tmp_file::cleanup_tmpfile(reader, input_file);

    Ok(Box::pin(clean_reader))
}

pub(super) async fn thumbnail<S: Store + 'static>(
    tmp_dir: &TmpDir,
    store: &S,
    identifier: &Arc<str>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
    timeout: u64,
) -> Result<BoxRead<'static>, MagickError> {
    let stream = store
        .to_stream(identifier, None, None)
        .await
        .map_err(MagickError::Store)?;

    thumbnail_animation(
        tmp_dir,
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
