use crate::magick::ValidInputType;
use clap::ValueEnum;
use std::{fmt::Display, path::PathBuf, str::FromStr};
use tracing::Level;
use url::Url;

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogFormat {
    Compact,
    Json,
    Normal,
    Pretty,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImageFormat {
    Jpeg,
    Webp,
    Png,
}

#[derive(Clone, Debug)]
pub(crate) struct Targets {
    pub(crate) targets: tracing_subscriber::filter::Targets,
}

/// Configuration for filesystem media storage
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, clap::Parser)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Filesystem {
    /// Path to store media
    #[arg(short, long)]
    pub(crate) path: PathBuf,
}

/// Configuration for object media storage
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, clap::Parser)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ObjectStorage {
    /// The base endpoint for the object storage
    ///
    /// Examples:
    /// - `http://localhost:9000`
    /// - `https://s3.dualstack.eu-west-1.amazonaws.com`
    #[arg(short, long)]
    pub(crate) endpoint: Url,

    /// Determines whether to use path style or virtualhost style for accessing objects
    ///
    /// When this is true, objects will be fetched from {endpoint}/{bucket_name}/{object}
    /// When false, objects will be fetched from {bucket_name}.{endpoint}/{object}
    #[arg(short, long)]
    pub(crate) use_path_style: bool,

    /// The bucket in which to store media
    #[arg(short, long)]
    pub(crate) bucket_name: String,

    /// The region the bucket is located in
    #[arg(short, long)]
    pub(crate) region: String,

    /// The Access Key for the user accessing the bucket
    #[arg(short, long)]
    pub(crate) access_key: String,

    /// The secret key for the user accessing the bucket
    #[arg(short, long)]
    pub(crate) secret_key: String,

    /// The session token for accessing the bucket
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_token: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub(crate) enum Store {
    Filesystem(Filesystem),

    ObjectStorage(ObjectStorage),
}

impl ImageFormat {
    pub(crate) fn as_hint(self) -> ValidInputType {
        ValidInputType::from_format(self)
    }

    pub(crate) fn as_magick_format(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Webp => "WEBP",
        }
    }

    pub(crate) fn as_ext(self) -> &'static str {
        match self {
            Self::Jpeg => ".jpeg",
            Self::Png => ".png",
            Self::Webp => ".webp",
        }
    }
}

impl From<Filesystem> for Store {
    fn from(f: Filesystem) -> Self {
        Self::Filesystem(f)
    }
}

impl From<ObjectStorage> for Store {
    fn from(o: ObjectStorage) -> Self {
        Self::ObjectStorage(o)
    }
}

impl FromStr for Targets {
    type Err = <tracing_subscriber::filter::Targets as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Targets {
            targets: s.parse()?,
        })
    }
}

impl Display for Targets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let targets = self
            .targets
            .iter()
            .map(|(path, level)| format!("{}={}", path, level))
            .collect::<Vec<_>>()
            .join(",");

        let max_level = [
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
        ]
        .iter()
        .fold(None, |found, level| {
            if found.is_none()
                && self
                    .targets
                    .would_enable("not_a_real_target_so_nothing_can_conflict", level)
            {
                Some(level.to_string().to_lowercase())
            } else {
                found
            }
        });

        if let Some(level) = max_level {
            if !targets.is_empty() {
                write!(f, "{},{}", level, targets)
            } else {
                write!(f, "{}", level)
            }
        } else if !targets.is_empty() {
            write!(f, "{}", targets)
        } else {
            Ok(())
        }
    }
}

impl FromStr for ImageFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "webp" => Ok(Self::Webp),
            other => Err(format!("Invalid variant: {}", other)),
        }
    }
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for variant in Self::value_variants() {
            if variant.to_possible_value().unwrap().matches(s, false) {
                return Ok(*variant);
            }
        }
        Err(format!("Invalid variant: {}", s))
    }
}

impl Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.to_possible_value()
            .expect("no values are skipped")
            .get_name()
            .fmt(f)
    }
}

impl Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.to_possible_value()
            .expect("no values are skipped")
            .get_name()
            .fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::Targets;
    use crate::serde_str::Serde;

    #[test]
    fn builds_info_targets() {
        let t: Serde<Targets> = "info".parse().unwrap();

        println!("{:?}", t);

        assert_eq!(t.to_string(), "info");
    }

    #[test]
    fn builds_specific_targets() {
        let t: Serde<Targets> = "pict_rs=info".parse().unwrap();

        assert_eq!(t.to_string(), "pict_rs=info");
    }

    #[test]
    fn builds_warn_and_specific_targets() {
        let t: Serde<Targets> = "warn,pict_rs=info".parse().unwrap();

        assert_eq!(t.to_string(), "warn,pict_rs=info");
    }
}
