use std::ffi::OsStr;

use crate::{
    formats::{AnimationFormat, ImageFormat},
    magick::{MagickError, MAGICK_CONFIGURE_PATH, MAGICK_TEMPORARY_PATH},
    process::Process,
    state::State,
};

pub(super) async fn convert_image_command<S>(
    state: &State<S>,
    input: ImageFormat,
    output: ImageFormat,
    quality: Option<u8>,
) -> Result<Process, MagickError> {
    convert(
        state,
        input.magick_format(),
        output.magick_format(),
        false,
        quality,
    )
    .await
}

pub(super) async fn convert_animation_command<S>(
    state: &State<S>,
    input: AnimationFormat,
    output: AnimationFormat,
    quality: Option<u8>,
) -> Result<Process, MagickError> {
    convert(
        state,
        input.magick_format(),
        output.magick_format(),
        true,
        quality,
    )
    .await
}

async fn convert<S>(
    state: &State<S>,
    input_format: &'static str,
    output_format: &'static str,
    coalesce: bool,
    quality: Option<u8>,
) -> Result<Process, MagickError> {
    let temporary_path = state
        .tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_arg = format!("{input_format}:-");

    let output_arg = format!("{output_format}:-");
    let quality = quality.map(|q| q.to_string());

    let mut args: Vec<&OsStr> = vec!["convert".as_ref()];

    if coalesce {
        args.push("-coalesce".as_ref());
    }

    args.extend([
        "-strip".as_ref(),
        "-auto-orient".as_ref(),
        input_arg.as_ref(),
    ] as [&OsStr; 3]);

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
