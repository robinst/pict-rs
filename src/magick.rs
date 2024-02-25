use std::{ffi::OsStr, ops::Deref, path::Path, sync::Arc};

use crate::{
    config::Media,
    error_code::ErrorCode,
    formats::ProcessableFormat,
    process::{Process, ProcessError},
    state::State,
    tmp_file::{TmpDir, TmpFolder},
};

pub(crate) const MAGICK_TEMPORARY_PATH: &str = "MAGICK_TEMPORARY_PATH";
pub(crate) const MAGICK_CONFIGURE_PATH: &str = "MAGICK_CONFIGURE_PATH";

#[derive(Debug, thiserror::Error)]
pub(crate) enum MagickError {
    #[error("Error in imagemagick process")]
    Process(#[source] ProcessError),

    #[error("Invalid output format: {0}")]
    Json(String, #[source] serde_json::Error),

    #[error("Error creating temporary directory")]
    CreateTemporaryDirectory(#[source] std::io::Error),

    #[error("Error in metadata discovery")]
    Discover(#[source] crate::discover::DiscoverError),

    #[error("Invalid media file provided")]
    CommandFailed(ProcessError),

    #[error("Error cleaning up after command")]
    Cleanup(#[source] std::io::Error),

    #[error("Command output is empty")]
    Empty,
}

impl From<ProcessError> for MagickError {
    fn from(value: ProcessError) -> Self {
        match value {
            e @ ProcessError::Status(_, _) => Self::CommandFailed(e),
            otherwise => Self::Process(otherwise),
        }
    }
}

impl MagickError {
    pub(crate) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::CommandFailed(_) => ErrorCode::COMMAND_FAILURE,
            Self::Process(e) => e.error_code(),
            Self::Json(_, _)
            | Self::CreateTemporaryDirectory(_)
            | Self::Discover(_)
            | Self::Cleanup(_)
            | Self::Empty => ErrorCode::COMMAND_ERROR,
        }
    }

    pub(crate) fn is_client_error(&self) -> bool {
        // Failing validation or imagemagick bailing probably means bad input
        matches!(
            self,
            Self::CommandFailed(_) | Self::Process(ProcessError::Timeout(_))
        )
    }
}

pub(crate) async fn process_image_command<S>(
    state: &State<S>,
    process_args: Vec<String>,
    input_format: ProcessableFormat,
    format: ProcessableFormat,
    quality: Option<u8>,
) -> Result<Process, MagickError> {
    let temporary_path = state
        .tmp_dir
        .tmp_folder()
        .await
        .map_err(MagickError::CreateTemporaryDirectory)?;

    let input_arg = format!("{}:-", input_format.magick_format());
    let output_arg = format!("{}:-", format.magick_format());
    let quality = quality.map(|q| q.to_string());

    let len = 3
        + if input_format.coalesce() { 1 } else { 0 }
        + if quality.is_some() { 1 } else { 0 }
        + process_args.len();

    let mut args: Vec<&OsStr> = Vec::with_capacity(len);
    args.push("convert".as_ref());
    args.push(input_arg.as_ref());
    if input_format.coalesce() {
        args.push("-coalesce".as_ref());
    }
    args.extend(process_args.iter().map(AsRef::<OsStr>::as_ref));
    if let Some(quality) = &quality {
        args.extend(["-quality".as_ref(), quality.as_ref()] as [&OsStr; 2]);
    }
    args.push(output_arg.as_ref());

    let envs = [
        (MAGICK_TEMPORARY_PATH, temporary_path.as_os_str()),
        (MAGICK_CONFIGURE_PATH, state.policy_dir.as_os_str()),
    ];

    let process = Process::run("magick", &args, &envs, state.config.media.process_timeout)?
        .add_extras(temporary_path);

    Ok(process)
}

pub(crate) type ArcPolicyDir = Arc<PolicyDir>;

pub(crate) struct PolicyDir {
    folder: TmpFolder,
}

impl PolicyDir {
    pub(crate) async fn cleanup(self: Arc<Self>) -> std::io::Result<()> {
        if let Some(this) = Arc::into_inner(self) {
            this.folder.cleanup().await?;
        }
        Ok(())
    }
}

impl AsRef<Path> for PolicyDir {
    fn as_ref(&self) -> &Path {
        &self.folder
    }
}

impl Deref for PolicyDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.folder
    }
}

pub(super) async fn write_magick_policy(
    media: &Media,
    tmp_dir: &TmpDir,
) -> std::io::Result<ArcPolicyDir> {
    let folder = tmp_dir.tmp_folder().await?;
    let file = folder.join("policy.xml");

    let res = tokio::fs::write(&file, generate_policy(media)).await;

    if let Err(e) = res {
        folder.cleanup().await?;
        return Err(e);
    }

    Ok(Arc::new(PolicyDir { folder }))
}

fn generate_policy(media: &Media) -> String {
    let width = media.magick.max_width;
    let height = media.magick.max_height;
    let area = media.magick.max_area;
    let memory = media.magick.memory;
    let map = media.magick.map;
    let disk = media.magick.disk;
    let frames = media.animation.max_frame_count;
    let timeout = media.process_timeout;

    format!(
        r#"<policymap>
    <policy domain="resource" name="width" value="{width}P" />
    <policy domain="resource" name="height" value="{height}P" />
    <policy domain="resource" name="area" value="{area}P" />
    <policy domain="resource" name="list-length" value="{frames}" />
    <policy domain="resource" name="time" value="{timeout}" />
    <policy domain="resource" name="memory" value="{memory}MiB" />
    <policy domain="resource" name="map" value="{map}MiB" />
    <policy domain="resource" name="disk" value="{disk}MiB" />
    <policy domain="resource" name="file" value="768" />
    <policy domain="resource" name="thread" value="2" />
    <policy domain="cache" name="memory-map" value="anonymous"/>
    <policy domain="cache" name="synchronize" value="true"/>
    <policy domain="path" rights="none" pattern="@*" />
    <policy domain="coder" rights="none" pattern="*" />
    <policy domain="coder" rights="read | write" pattern="{{APNG,AVIF,GIF,HEIC,JPEG,JSON,JXL,PNG,RGB,RGBA,WEBP,MP4,WEBM,TMP,PAM}}" />
    <policy domain="delegate" rights="none" pattern="*" />
    <policy domain="delegate" rights="execute" pattern="FFMPEG" />
    <policy domain="filter" rights="none" pattern="*" />
    <policy domain="module" rights="none" pattern="*" />
    <policy domain="module" rights="read | write" pattern="{{APNG,AVIF,GIF,HEIC,JPEG,JSON,JXL,PNG,RGB,RGBA,WEBP,TMP,PAM,PNM,VIDEO}}" />
        <!-- indirect reads not permitted -->
    <policy domain="system" name="max-memory-request" value="256MiB"/>
    <policy domain="system" name="memory-map" value="anonymous"/>
    <policy domain="system" name="precision" value="6" />
</policymap>"#
    )
}
