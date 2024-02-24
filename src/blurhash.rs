use std::ffi::OsStr;

use tokio::io::AsyncReadExt;

use crate::{
    details::Details,
    error::{Error, UploadError},
    formats::ProcessableFormat,
    magick::{MagickError, MAGICK_CONFIGURE_PATH, MAGICK_TEMPORARY_PATH},
    process::{Process, ProcessRead},
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

    let process = read_rgba(
        state,
        input_details
            .internal_format()
            .processable_format()
            .expect("not a video"),
        |mut tmp_file| async move {
            tmp_file
                .write_from_stream(stream)
                .await
                .map_err(MagickError::Write)?;
            Ok(tmp_file)
        },
    )
    .await?;

    let blurhash = process
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

async fn read_rgba<S, F, Fut>(
    state: &State<S>,
    input_format: ProcessableFormat,
    write_file: F,
) -> Result<ProcessRead, MagickError>
where
    F: FnOnce(crate::file::File) -> Fut,
    Fut: std::future::Future<Output = Result<crate::file::File, MagickError>>,
{
    let temporary_path = state
        .tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_file = state.tmp_dir.tmp_file(None);
    crate::store::file_store::safe_create_parent(&input_file)
        .await
        .map_err(MagickError::CreateDir)?;

    let tmp_one = crate::file::File::create(&input_file)
        .await
        .map_err(MagickError::CreateFile)?;
    let tmp_one = (write_file)(tmp_one).await?;
    tmp_one.close().await.map_err(MagickError::CloseFile)?;

    let mut input_arg = [
        input_format.magick_format().as_ref(),
        input_file.as_os_str(),
    ]
    .join(":".as_ref());
    if input_format.coalesce() {
        input_arg.push("[0]");
    }

    let args: [&OsStr; 3] = ["convert".as_ref(), &input_arg, "RGBA:-".as_ref()];

    let envs = [
        (MAGICK_TEMPORARY_PATH, temporary_path.as_os_str()),
        (MAGICK_CONFIGURE_PATH, state.policy_dir.as_os_str()),
    ];

    let process = Process::run("magick", &args, &envs, state.config.media.process_timeout)?
        .read()
        .add_extras(input_file)
        .add_extras(temporary_path);

    Ok(process)
}
