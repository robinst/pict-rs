use crate::error::{Error, UploadError};
use std::path::PathBuf;

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
pub(crate) struct Resize {
    filter: Option<ResizeFilter>,
    kind: ResizeKind,
}
#[derive(Copy, Clone)]
pub(crate) enum ResizeFilter {
    Lanczos,
    LanczosSharp,
    Lanczos2,
    Lanczos2Sharp,
    Mitchell,
    RobidouxSharp,
}
#[derive(Copy, Clone)]
pub(crate) enum ResizeKind {
    Bounds(usize),
    Area(usize),
}
pub(crate) struct Crop(usize, usize);
pub(crate) struct Blur(f64);

impl ResizeFilter {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "lanczos" => Some(Self::Lanczos),
            "lanczossharp" => Some(Self::LanczosSharp),
            "lanczos2" => Some(Self::Lanczos2),
            "lanczos2sharp" => Some(Self::Lanczos2Sharp),
            "mitchell" => Some(Self::Mitchell),
            "robidouxsharp" => Some(Self::RobidouxSharp),
            _ => None,
        }
    }

    fn to_magick_str(self) -> &'static str {
        match self {
            Self::Lanczos => "Lanczos",
            Self::LanczosSharp => "LanczosSharp",
            Self::Lanczos2 => "Lanczos2",
            Self::Lanczos2Sharp => "Lanczos2Sharp",
            Self::Mitchell => "Mitchell",
            Self::RobidouxSharp => "RobidouxSharp",
        }
    }
}

impl Default for ResizeFilter {
    fn default() -> Self {
        Self::Lanczos2
    }
}

impl ResizeKind {
    fn from_str(s: &str) -> Option<Self> {
        let kind = if s.starts_with('a') {
            let size = s.trim_start_matches('a').parse().ok()?;
            ResizeKind::Area(size)
        } else {
            let size = s.parse().ok()?;
            ResizeKind::Bounds(size)
        };

        Some(kind)
    }

    fn to_magick_string(self) -> String {
        match self {
            Self::Area(size) => format!("{size}@>"),
            Self::Bounds(size) => format!("{size}x{size}>"),
        }
    }
}

#[tracing::instrument(level = "debug")]
pub(crate) fn build_chain(
    args: &[(String, String)],
    ext: &str,
) -> Result<(PathBuf, Vec<String>), Error> {
    fn parse<P: Processor>(key: &str, value: &str) -> Result<Option<P>, Error> {
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

    let (mut path, args) =
        args.iter()
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

    path.push(ext);

    Ok((path, args))
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
        if let Some((first, second)) = v.split_once('.') {
            let filter = ResizeFilter::from_str(first);

            let kind = ResizeKind::from_str(second)?;

            Some(Resize { filter, kind })
        } else {
            let size = v.parse().ok()?;
            Some(Resize {
                filter: None,
                kind: ResizeKind::Bounds(size),
            })
        }
    }

    fn path(&self, mut path: PathBuf) -> PathBuf {
        path.push(Self::NAME);
        match self {
            Resize {
                filter: None,
                kind: ResizeKind::Bounds(size),
            } => {
                path.push(size.to_string());
            }
            Resize {
                filter: None,
                kind: ResizeKind::Area(size),
            } => {
                let node = format!(".a{size}");
                path.push(node);
            }
            Resize {
                filter: Some(filter),
                kind: ResizeKind::Bounds(size),
            } => {
                let node = format!("{}.{size}", filter.to_magick_str());
                path.push(node);
            }
            Resize {
                filter: Some(filter),
                kind: ResizeKind::Area(size),
            } => {
                let node = format!("{}.a{size}", filter.to_magick_str());
                path.push(node);
            }
        }
        path
    }

    fn command(&self, mut args: Vec<String>) -> Vec<String> {
        args.extend([
            "-filter".to_string(),
            self.filter.unwrap_or_default().to_magick_str().to_string(),
            "-resize".to_string(),
            self.kind.to_magick_string(),
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
