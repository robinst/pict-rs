use std::ffi::{OsStr, OsString};

use tokio::io::AsyncReadExt;

use crate::{
    details::Details,
    error::{Error, UploadError},
    formats::ProcessableFormat,
    magick::{MagickError, MAGICK_CONFIGURE_PATH, MAGICK_TEMPORARY_PATH},
    process::Process,
    repo::Alias,
    state::State,
    store::Store,
};

pub(crate) async fn generate<S>(
    state: &State<S>,
    alias: &Alias,
    original_details: &Details,
) -> Result<String, Error>
where
    S: Store + 'static,
{
    let hash = state
        .repo
        .hash(alias)
        .await?
        .ok_or(UploadError::MissingIdentifier)?;

    let identifier = if original_details.is_video() {
        crate::generate::ensure_motion_identifier(state, hash, original_details).await?
    } else {
        state
            .repo
            .identifier(hash)
            .await?
            .ok_or(UploadError::MissingIdentifier)?
    };

    let input_details = crate::ensure_details_identifier(state, &identifier).await?;

    let stream = state.store.to_stream(&identifier, None, None).await?;

    let blurhash = read_rgba_command(
        state,
        input_details
            .internal_format()
            .processable_format()
            .expect("not a video"),
    )
    .await?
    .drive_with_stream(stream)
    .with_stdout(|mut stdout| async move {
        let mut encoder = blurhash_update::Encoder::auto(blurhash_update::ImageBounds {
            width: input_details.width() as _,
            height: input_details.height() as _,
        });

        let mut buf = [0u8; 1024 * 8];

        loop {
            let n = stdout.read(&mut buf).await?;

            if n == 0 {
                break;
            }

            encoder.update(&buf[..n]);
        }

        Ok(encoder.finalize()) as std::io::Result<String>
    })
    .await??;

    Ok(blurhash)
}

async fn read_rgba_command<S>(
    state: &State<S>,
    input_format: ProcessableFormat,
) -> Result<Process, MagickError> {
    let temporary_path = state
        .tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let mut input_arg = OsString::from(input_format.magick_format());
    input_arg.push(":-");
    if input_format.coalesce() {
        input_arg.push("[0]");
    }

    let args: [&OsStr; 3] = ["convert".as_ref(), &input_arg, "RGBA:-".as_ref()];

    let envs = [
        (MAGICK_TEMPORARY_PATH, temporary_path.as_os_str()),
        (MAGICK_CONFIGURE_PATH, state.policy_dir.as_os_str()),
    ];

    let process = Process::run("magick", &args, &envs, state.config.media.process_timeout)?
        .add_extras(temporary_path);

    Ok(process)
}
