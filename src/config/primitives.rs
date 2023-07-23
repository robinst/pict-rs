use clap::ValueEnum;
use std::{fmt::Display, path::PathBuf, str::FromStr};
use tracing::Level;
use url::Url;

#[derive(Clone, Debug)]
pub(crate) struct RetentionValue {
    value: u32,
    units: TimeUnit,
}

#[derive(Clone, Debug)]
enum TimeUnit {
    Minute,
    Hour,
    Day,
    Year,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RetentionValueError {
    #[error("No number in retention value")]
    NoValue,
    #[error("Provided value is invalid")]
    InvalidValue,
    #[error("No units in retention value")]
    NoUnits,
    #[error("Provided units are invalid")]
    InvalidUnits,
}

impl RetentionValue {
    pub(crate) fn to_duration(&self) -> time::Duration {
        match self.units {
            TimeUnit::Minute => time::Duration::minutes(i64::from(self.value)),
            TimeUnit::Hour => time::Duration::hours(i64::from(self.value)),
            TimeUnit::Day => time::Duration::days(i64::from(self.value)),
            TimeUnit::Year => time::Duration::days(i64::from(self.value) * 365),
        }
    }
}

impl std::str::FromStr for RetentionValue {
    type Err = RetentionValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let num_str = s
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>();

        if num_str.is_empty() {
            return Err(RetentionValueError::NoValue);
        }

        let value: u32 = num_str
            .parse()
            .map_err(|_| RetentionValueError::InvalidValue)?;

        let units = s.trim_start_matches(&num_str).to_lowercase();

        if units.is_empty() {
            return Err(RetentionValueError::NoUnits);
        }

        let units = match units.as_str() {
            "m" | "minute" => TimeUnit::Minute,
            "h" | "hour" => TimeUnit::Hour,
            "d" | "day" => TimeUnit::Day,
            "y" | "year" => TimeUnit::Year,
            _ => return Err(RetentionValueError::InvalidUnits),
        };

        Ok(RetentionValue { value, units })
    }
}

impl std::fmt::Display for TimeUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Minute => write!(f, "m"),
            Self::Hour => write!(f, "h"),
            Self::Day => write!(f, "d"),
            Self::Year => write!(f, "y"),
        }
    }
}

impl std::fmt::Display for RetentionValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.value, self.units)
    }
}

impl<'de> serde::Deserialize<'de> for RetentionValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let s = String::deserialize(deserializer)?;

        s.parse().map_err(D::Error::custom)
    }
}

impl serde::Serialize for RetentionValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = self.to_string();

        s.serialize(serializer)
    }
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
pub(crate) enum LogFormat {
    Compact,
    Json,
    Normal,
    Pretty,
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

    /// How long signatures for object storage requests are valid (in seconds)
    ///
    /// This defaults to 15 seconds
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) signature_duration: Option<u64>,

    /// How long a client can wait on an object storage request before giving up (in seconds)
    ///
    /// This defaults to 30 seconds
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) client_timeout: Option<u64>,

    /// Base endpoint at which object storage images are publicly accessible
    #[arg(long)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) public_endpoint: Option<Url>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
// allow large enum variant - this is an instantiated-once config
#[allow(clippy::large_enum_variant)]
pub(crate) enum Store {
    Filesystem(Filesystem),

    ObjectStorage(ObjectStorage),
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
            .map(|(path, level)| format!("{path}={level}"))
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
                write!(f, "{level},{targets}")
            } else {
                write!(f, "{level}")
            }
        } else if !targets.is_empty() {
            write!(f, "{targets}")
        } else {
            Ok(())
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
        Err(format!("Invalid variant: {s}"))
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
