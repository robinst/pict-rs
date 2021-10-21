use crate::{
    error::{Error, UploadError},
    ffmpeg::ThumbnailFormat,
};
use std::path::{Path, PathBuf};
use tracing::{debug, error, instrument};

fn ptos(path: &Path) -> Result<String, Error> {
    Ok(path.to_str().ok_or(UploadError::Path)?.to_owned())
}

pub(crate) trait Processor {
    const NAME: &'static str;

    fn parse(k: &str, v: &str) -> Option<Self>
    where
        Self: Sized;

    fn path(&self, path: PathBuf) -> PathBuf;
    fn command(&self, args: Vec<String>) -> Vec<String>;
}

pub(crate) struct Identity;

impl Processor for Identity {
    const NAME: &'static str = "identity";

    fn parse(_: &str, _: &str) -> Option<Self>
    where
        Self: Sized,
    {
        Some(Identity)
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
    const NAME: &'static str = "thumbnail";

    fn parse(_: &str, v: &str) -> Option<Self>
    where
        Self: Sized,
    {
        let size = v.parse().ok()?;
        Some(Thumbnail(size))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::NAME);
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
    const NAME: &'static str = "resize";

    fn parse(_: &str, v: &str) -> Option<Self>
    where
        Self: Sized,
    {
        let size = v.parse().ok()?;
        Some(Resize(size))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::NAME);
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
    const NAME: &'static str = "crop";

    fn parse(_: &str, v: &str) -> Option<Self> {
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

        Some(Crop(width, height))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::NAME);
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
    const NAME: &'static str = "blur";

    fn parse(_: &str, v: &str) -> Option<Self> {
        let sigma = v.parse().ok()?;
        Some(Blur(sigma))
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::NAME);
        path.push(self.0.to_string());
        path
    }

    fn command(&self, mut args: Vec<String>) -> Vec<String> {
        args.extend(["-gaussian-blur".to_string(), self.0.to_string()]);

        args
    }
}

trait Process {
    fn path(&self, path: PathBuf) -> PathBuf;

    fn command(&self, args: Vec<String>) -> Vec<String>;

    fn len(&self, len: u64) -> u64;
}

#[derive(Debug)]
struct Base;

#[derive(Debug)]
struct ProcessorNode<Inner, P> {
    inner: Inner,
    processor: P,
}

impl<Inner, P> ProcessorNode<Inner, P>
where
    P: Processor,
{
    fn new(inner: Inner, processor: P) -> Self {
        ProcessorNode { inner, processor }
    }
}

impl Process for Base {
    fn path(&self, path: PathBuf) -> PathBuf {
        path
    }

    fn command(&self, args: Vec<String>) -> Vec<String> {
        args
    }

    fn len(&self, len: u64) -> u64 {
        len
    }
}

impl<Inner, P> Process for ProcessorNode<Inner, P>
where
    Inner: Process,
    P: Processor,
{
    fn path(&self, path: PathBuf) -> PathBuf {
        self.processor.path(self.inner.path(path))
    }

    fn command(&self, args: Vec<String>) -> Vec<String> {
        self.processor.command(self.inner.command(args))
    }

    fn len(&self, len: u64) -> u64 {
        self.inner.len(len + 1)
    }
}

struct ProcessChain<P> {
    inner: P,
}

impl<P> ProcessChain<P>
where
    P: Process,
{
    fn len(&self) -> u64 {
        self.inner.len(0)
    }

    fn command(&self) -> Vec<String> {
        self.inner.command(vec![])
    }

    fn path(&self) -> PathBuf {
        self.inner.path(PathBuf::new())
    }
}

impl<P> std::fmt::Debug for ProcessChain<P>
where
    P: Process,
{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("ProcessChain")
            .field("path", &self.path())
            .field("command", &self.command())
            .field("steps", &self.len())
            .finish()
    }
}

#[instrument]
pub(crate) fn build_chain(args: &[(String, String)], filename: String) -> (PathBuf, Vec<String>) {
    fn parse<P: Processor>(key: &str, value: &str) -> Option<P> {
        if key == P::NAME {
            return P::parse(key, value);
        }

        None
    }

    macro_rules! parse {
        ($inner:expr, $args:expr, $filename:expr, $x:ident, $k:expr, $v:expr) => {{
            if let Some(processor) = parse::<$x>($k, $v) {
                return build(
                    ProcessorNode::new($inner, processor),
                    &$args[1..],
                    $filename,
                );
            }
        }};
    }

    fn build(
        inner: impl Process,
        args: &[(String, String)],
        filename: String,
    ) -> (PathBuf, Vec<String>) {
        if args.len() == 0 {
            let chain = ProcessChain { inner };

            debug!("built: {:?}", chain);

            return (chain.path().join(filename), chain.command());
        }

        let (name, value) = &args[0];

        parse!(inner, args, filename, Identity, name, value);
        parse!(inner, args, filename, Thumbnail, name, value);
        parse!(inner, args, filename, Resize, name, value);
        parse!(inner, args, filename, Crop, name, value);
        parse!(inner, args, filename, Blur, name, value);

        debug!("Skipping {}: {}, invalid", name, value);

        build(inner, &args[1..], filename)
    }

    build(Base, args, filename)
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
) -> Result<Option<(PathBuf, Exists)>, Error> {
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
            return Err(e);
        }

        return match crate::safe_move_file(tmpfile, jpg_path.clone()).await {
            Err(e) if matches!(e.kind(), UploadError::FileExists) => {
                Ok(Some((jpg_path, Exists::Exists)))
            }
            Err(e) => Err(e),
            _ => Ok(Some((jpg_path, Exists::New))),
        };
    }

    Ok(None)
}
