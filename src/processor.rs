use crate::{error::UploadError, ffmpeg::ThumbnailFormat};
use std::path::{Path, PathBuf};
use tracing::{debug, error, instrument};

fn ptos(path: &Path) -> Result<String, UploadError> {
    Ok(path.to_str().ok_or(UploadError::Path)?.to_owned())
}

pub(crate) trait Processor {
    fn name() -> &'static str
    where
        Self: Sized;

    fn is_processor(s: &str) -> bool
    where
        Self: Sized;

    fn parse(k: &str, v: &str) -> Option<Box<dyn Processor + Send>>
    where
        Self: Sized;

    fn path(&self, path: PathBuf) -> PathBuf;
    fn command(&self, args: Vec<String>) -> Vec<String>;
}

pub(crate) struct Identity;

impl Processor for Identity {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "identity"
    }

    fn is_processor(s: &str) -> bool
    where
        Self: Sized,
    {
        s == Self::name()
    }

    fn parse(_: &str, _: &str) -> Option<Box<dyn Processor + Send>>
    where
        Self: Sized,
    {
        Some(Box::new(Identity))
    }

    fn path(&self, path: PathBuf) -> PathBuf {
        path
    }

    fn command(&self, args: Vec<String>) -> Vec<String> {
        args
    }
}

pub(crate) struct Thumbnail(usize);

impl Processor for Thumbnail {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "thumbnail"
    }

    fn is_processor(s: &str) -> bool
    where
        Self: Sized,
    {
        s == Self::name()
    }

    fn parse(_: &str, v: &str) -> Option<Box<dyn Processor + Send>>
    where
        Self: Sized,
    {
        let size = v.parse().ok()?;
        Some(Box::new(Thumbnail(size)))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::name());
        path.push(self.0.to_string());
        path
    }

    fn command(&self, mut args: Vec<String>) -> Vec<String> {
        args.extend(["-sample".to_string(), format!("{}x{}>", self.0, self.0)]);

        args
    }
}

pub(crate) struct Resize(usize);

impl Processor for Resize {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "resize"
    }

    fn is_processor(s: &str) -> bool
    where
        Self: Sized,
    {
        s == Self::name()
    }

    fn parse(_: &str, v: &str) -> Option<Box<dyn Processor + Send>>
    where
        Self: Sized,
    {
        let size = v.parse().ok()?;
        Some(Box::new(Resize(size)))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::name());
        path.push(self.0.to_string());
        path
    }

    fn command(&self, mut args: Vec<String>) -> Vec<String> {
        args.extend([
            "-filter".to_string(),
            "Lanczos2".to_string(),
            "-resize".to_string(),
            format!("{}x{}>", self.0, self.0),
        ]);

        args
    }
}

pub(crate) struct Crop(usize, usize);

impl Processor for Crop {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "crop"
    }

    fn is_processor(s: &str) -> bool {
        s == Self::name()
    }

    fn parse(_: &str, v: &str) -> Option<Box<dyn Processor + Send>> {
        let mut iter = v.split('x');
        let first = iter.next()?;
        let second = iter.next()?;

        let width = first.parse::<usize>().ok()?;
        let height = second.parse::<usize>().ok()?;

        if width == 0 || height == 0 {
            return None;
        }

        if width > 20 || height > 20 {
            return None;
        }

        Some(Box::new(Crop(width, height)))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::name());
        path.push(format!("{}x{}", self.0, self.1));
        path
    }

    fn command(&self, mut args: Vec<String>) -> Vec<String> {
        args.extend([
            "-gravity".to_string(),
            "center".to_string(),
            "-crop".to_string(),
            format!("{}:{}+0+0", self.0, self.1),
        ]);

        args
    }
}

pub(crate) struct Blur(f64);

impl Processor for Blur {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "blur"
    }

    fn is_processor(s: &str) -> bool {
        s == Self::name()
    }

    fn parse(_: &str, v: &str) -> Option<Box<dyn Processor + Send>> {
        let sigma = v.parse().ok()?;
        Some(Box::new(Blur(sigma)))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::name());
        path.push(self.0.to_string());
        path
    }

    fn command(&self, mut args: Vec<String>) -> Vec<String> {
        args.extend(["-gaussian-blur".to_string(), self.0.to_string()]);

        args
    }
}

macro_rules! parse {
    ($x:ident, $k:expr, $v:expr) => {{
        if $x::is_processor($k) {
            return $x::parse($k, $v);
        }
    }};
}

pub(crate) struct ProcessChain {
    inner: Vec<Box<dyn Processor + Send>>,
}

impl std::fmt::Debug for ProcessChain {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("ProcessChain")
            .field("steps", &self.inner.len())
            .finish()
    }
}

#[instrument]
pub(crate) fn build_chain(args: &[(String, String)]) -> ProcessChain {
    let inner = args
        .iter()
        .filter_map(|(k, v)| {
            let k = k.as_str();
            let v = v.as_str();

            parse!(Identity, k, v);
            parse!(Thumbnail, k, v);
            parse!(Resize, k, v);
            parse!(Crop, k, v);
            parse!(Blur, k, v);

            debug!("Skipping {}: {}, invalid", k, v);

            None
        })
        .collect();

    ProcessChain { inner }
}

pub(crate) fn build_path(base: PathBuf, chain: &ProcessChain, filename: String) -> PathBuf {
    let mut path = chain
        .inner
        .iter()
        .fold(base, |acc, processor| processor.path(acc));

    path.push(filename);
    path
}

pub(crate) fn build_args(chain: &ProcessChain) -> Vec<String> {
    chain
        .inner
        .iter()
        .fold(Vec::new(), |acc, processor| processor.command(acc))
}

fn is_motion(s: &str) -> bool {
    s.ends_with(".gif") || s.ends_with(".mp4")
}

pub(crate) enum Exists {
    Exists,
    New,
}

impl Exists {
    pub(crate) fn is_new(&self) -> bool {
        matches!(self, Exists::New)
    }
}

pub(crate) async fn prepare_image(
    original_file: PathBuf,
) -> Result<Option<(PathBuf, Exists)>, UploadError> {
    let original_path_str = ptos(&original_file)?;
    let jpg_path = format!("{}.jpg", original_path_str);
    let jpg_path = PathBuf::from(jpg_path);

    if tokio::fs::metadata(&jpg_path).await.is_ok() {
        return Ok(Some((jpg_path, Exists::Exists)));
    }

    if is_motion(&original_path_str) {
        let orig_path = original_path_str.clone();

        let tmpfile = crate::tmp_file();
        crate::safe_create_parent(&tmpfile).await?;

        let res = crate::ffmpeg::thumbnail(orig_path, &tmpfile, ThumbnailFormat::Jpeg).await;

        if let Err(e) = res {
            error!("transcode error: {:?}", e);
            tokio::fs::remove_file(&tmpfile).await?;
            return Err(e.into());
        }

        return match crate::safe_move_file(tmpfile, jpg_path.clone()).await {
            Err(UploadError::FileExists) => Ok(Some((jpg_path, Exists::Exists))),
            Err(e) => Err(e),
            _ => Ok(Some((jpg_path, Exists::New))),
        };
    }

    Ok(None)
}
