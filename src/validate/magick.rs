use std::ffi::OsStr;



use crate::{
    bytes_stream::BytesStream,
    formats::{AnimationFormat, ImageFormat},
    magick::{MagickError, MAGICK_CONFIGURE_PATH, MAGICK_TEMPORARY_PATH},
    process::{Process, ProcessRead},
    state::State,
};

pub(super) async fn convert_image<S>(
    state: &State<S>,
    input: ImageFormat,
    output: ImageFormat,
    quality: Option<u8>,
    bytes: BytesStream,
) -> Result<ProcessRead, MagickError> {
    convert(
        state,
        input.magick_format(),
        output.magick_format(),
        false,
        quality,
        bytes,
    )
    .await
}

pub(super) async fn convert_animation<S>(
    state: &State<S>,
    input: AnimationFormat,
    output: AnimationFormat,
    quality: Option<u8>,
    bytes: BytesStream,
) -> Result<ProcessRead, MagickError> {
    convert(
        state,
        input.magick_format(),
        output.magick_format(),
        true,
        quality,
        bytes,
    )
    .await
}

async fn convert<S>(
    state: &State<S>,
    input: &'static str,
    output: &'static str,
    coalesce: bool,
    quality: Option<u8>,
    bytes: BytesStream,
) -> Result<ProcessRead, MagickError> {
    let temporary_path = state
        .tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_file = state.tmp_dir.tmp_file(None);

    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let mut tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    tmp_one
        .write_from_stream(bytes.into_io_stream())
        .await
        .map_err(MagickError::Write)?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let input_arg = [input.as_ref(), input_file.as_os_str()].join(":".as_ref());
    let output_arg = format!("{output}:-");
    let quality = quality.map(|q| q.to_string());

    let mut args: Vec<&OsStr> = vec!["convert".as_ref()];

    if coalesce {
        args.push("-coalesce".as_ref());
    }

    args.extend(["-strip".as_ref(), "-auto-orient".as_ref(), &input_arg] as [&OsStr; 3]);

    if let Some(quality) = &quality {
        args.extend(["-quality".as_ref(), quality.as_ref()] as [&OsStr; 2]);
    }

    args.push(output_arg.as_ref());

    let envs = [
        (MAGICK_TEMPORARY_PATH, temporary_path.as_os_str()),
        (MAGICK_CONFIGURE_PATH, state.policy_dir.as_os_str()),
    ];

    let reader = Process::run("magick", &args, &envs, state.config.media.process_timeout)?.read();

    let clean_reader = reader.add_extras(input_file).add_extras(temporary_path);

    Ok(clean_reader)
}
