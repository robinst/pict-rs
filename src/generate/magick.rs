use std::ffi::OsStr;

use crate::{
    formats::{ImageFormat, ProcessableFormat},
    magick::{MagickError, MAGICK_CONFIGURE_PATH, MAGICK_TEMPORARY_PATH},
    process::Process,
    state::State,
};

pub(super) async fn thumbnail_command<S>(
    state: &State<S>,
    input_format: ProcessableFormat,
    thumbnail_format: ImageFormat,
) -> Result<Process, MagickError> {
    let format = ProcessableFormat::Image(thumbnail_format);
    let quality = state.config.media.image.quality_for(thumbnail_format);

    let temporary_path = state
        .tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_arg = format!("{}:-", input_format.magick_format());
    let output_arg = format!("{}:-", format.magick_format());
    let quality = quality.map(|q| q.to_string());

    let len = 3 + if format.coalesce() { 1 } else { 0 } + if quality.is_some() { 1 } else { 0 };

    let mut args: Vec<&OsStr> = Vec::with_capacity(len);
    args.push("convert".as_ref());
    args.push(input_arg.as_ref());
    if format.coalesce() {
        args.push("-coalesce".as_ref());
    }
    if let Some(quality) = &quality {
        args.extend(["-quality".as_ref(), quality.as_ref()] as [&OsStr; 2]);
    }
    args.push(output_arg.as_ref());

    let envs = [
        (MAGICK_TEMPORARY_PATH, temporary_path.as_os_str()),
        (MAGICK_CONFIGURE_PATH, state.policy_dir.as_os_str()),
    ];

    let process = Process::run("magick", &args, &envs, state.config.media.process_timeout)
        .await?
        .add_extras(temporary_path);

    Ok(process)
}
