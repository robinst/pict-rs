use clap::ArgEnum;
use std::{fmt::Display, str::FromStr};

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    ArgEnum,
)]
pub(crate) enum LogFormat {
    Compact,
    Json,
    Normal,
    Pretty,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    ArgEnum,
)]
pub(crate) enum ImageFormat {
    Jpeg,
    Webp,
    Png,
}

#[derive(Clone, Debug)]
pub(crate) struct Targets {
    pub(crate) targets: tracing_subscriber::filter::Targets,
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

        write!(f, "{}", targets)
    }
}

impl FromStr for ImageFormat {
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
