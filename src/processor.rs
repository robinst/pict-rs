use crate::{
    error::UploadError,
    validate::{ptos, Op},
};
use actix_web::web;
use bytes::Bytes;
use magick_rust::MagickWand;
use std::path::PathBuf;
use tracing::{debug, instrument, Span};

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
    fn process(&self, wand: &mut MagickWand) -> Result<(), UploadError>;
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
        debug!("Identity");
        Some(Box::new(Identity))
    }

    fn path(&self, path: PathBuf) -> PathBuf {
        path
    }

    fn process(&self, _: &mut MagickWand) -> Result<(), UploadError> {
        Ok(())
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

    fn process(&self, wand: &mut MagickWand) -> Result<(), UploadError> {
        debug!("Thumbnail");
        let width = wand.get_image_width();
        let height = wand.get_image_height();

        if width > self.0 || height > self.0 {
            let width_ratio = width as f64 / self.0 as f64;
            let height_ratio = height as f64 / self.0 as f64;

            let (new_width, new_height) = if width_ratio < height_ratio {
                (width as f64 / height_ratio, self.0 as f64)
            } else {
                (self.0 as f64, height as f64 / width_ratio)
            };

            wand.op(|w| w.sample_image(new_width as usize, new_height as usize))?;
        }

        Ok(())
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

    fn process(&self, wand: &mut MagickWand) -> Result<(), UploadError> {
        debug!("Blur");
        if self.0 > 0.0 {
            wand.op(|w| w.gaussian_blur_image(0.0, self.0))?;
        }

        Ok(())
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
        .into_iter()
        .filter_map(|(k, v)| {
            let k = k.as_str();
            let v = v.as_str();

            parse!(Identity, k, v);
            parse!(Thumbnail, k, v);
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

fn is_motion(s: &str) -> bool {
    s.ends_with(".gif") || s.ends_with(".mp4")
}

pub(crate) enum Exists {
    Exists,
    New,
}

impl Exists {
    pub(crate) fn is_new(&self) -> bool {
        match self {
            Exists::New => true,
            _ => false,
        }
    }
}

pub(crate) async fn prepare_image(
    original_file: PathBuf,
) -> Result<Option<(PathBuf, Exists)>, UploadError> {
    let original_path_str = ptos(&original_file)?;
    let jpg_path = format!("{}.jpg", original_path_str);
    let jpg_path = PathBuf::from(jpg_path);

    if actix_fs::metadata(jpg_path.clone()).await.is_ok() {
        return Ok(Some((jpg_path, Exists::Exists)));
    }

    if is_motion(&original_path_str) {
        let orig_path = original_path_str.clone();

        let tmpfile = crate::tmp_file();
        let tmpfile2 = tmpfile.clone();

        let res = web::block(move || {
            use crate::validate::transcode::{transcode, Target};

            transcode(orig_path, tmpfile, Target::Jpeg).map_err(UploadError::Transcode)
        })
        .await;

        if let Err(e) = res {
            actix_fs::remove_file(tmpfile2).await?;
            return Err(e.into());
        }

        return match crate::safe_move_file(tmpfile2, jpg_path.clone()).await {
            Err(UploadError::FileExists) => Ok(Some((jpg_path, Exists::Exists))),
            Err(e) => Err(e),
            _ => Ok(Some((jpg_path, Exists::New))),
        };
    }

    Ok(None)
}

#[instrument]
pub(crate) async fn process_image(
    original_file: PathBuf,
    chain: ProcessChain,
) -> Result<Bytes, UploadError> {
    let original_path_str = ptos(&original_file)?;

    let span = Span::current();
    let bytes = web::block(move || {
        let entered = span.enter();

        let mut wand = MagickWand::new();
        debug!("Reading image");
        wand.op(|w| w.read_image(&original_path_str))?;

        let format = wand.op(|w| w.get_image_format())?;

        debug!("Processing image");
        for processor in chain.inner.into_iter() {
            debug!("Step");
            processor.process(&mut wand)?;
        }

        let vec = wand.op(|w| w.write_image_blob(&format))?;
        drop(entered);
        return Ok(Bytes::from(vec)) as Result<Bytes, UploadError>;
    })
    .await?;

    Ok(bytes)
}
