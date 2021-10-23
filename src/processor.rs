use crate::error::{Error, UploadError};
use std::path::PathBuf;
use tracing::instrument;

pub(crate) trait Processor {
    const NAME: &'static str;

    fn parse(k: &str, v: &str) -> Option<Self>
    where
        Self: Sized;

    fn path(&self, path: PathBuf) -> PathBuf;
    fn command(&self, args: Vec<String>) -> Vec<String>;
}

pub(crate) struct Identity;
pub(crate) struct Thumbnail(usize);
pub(crate) struct Resize(usize);
pub(crate) struct Crop(usize, usize);
pub(crate) struct Blur(f64);

#[instrument]
pub(crate) fn build_chain(
    args: &[(String, String)],
    filename: String,
) -> Result<(PathBuf, Vec<String>), Error> {
    fn parse<P: Processor>(key: &str, value: &str) -> Result<Option<P>, UploadError> {
        if key == P::NAME {
            return Ok(Some(P::parse(key, value).ok_or(UploadError::ParsePath)?));
        }

        Ok(None)
    }

    macro_rules! parse {
        ($inner:expr, $x:ident, $k:expr, $v:expr) => {{
            if let Some(processor) = parse::<$x>($k, $v)? {
                return Ok((processor.path($inner.0), processor.command($inner.1)));
            };
        }};
    }

    let (path, args) =
        args.into_iter()
            .fold(Ok((PathBuf::default(), vec![])), |inner, (name, value)| {
                if let Ok(inner) = inner {
                    parse!(inner, Identity, name, value);
                    parse!(inner, Thumbnail, name, value);
                    parse!(inner, Resize, name, value);
                    parse!(inner, Crop, name, value);
                    parse!(inner, Blur, name, value);

                    Err(Error::from(UploadError::ParsePath))
                } else {
                    inner
                }
            })?;

    Ok((path.join(filename), args))
}

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
