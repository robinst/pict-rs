use clap::Parser;
use std::path::{Path, PathBuf};

mod commandline;
mod defaults;
mod file;
pub mod primitives;

use commandline::{Args, Output};
use config::Config;
use defaults::Defaults;

pub(crate) use commandline::Operation;
pub(crate) use file::{
    Animation, ConfigFile as Configuration, Image, Media, ObjectStorage, OpenTelemetry, Repo, Sled,
    Store, Tracing, Video,
};
pub(crate) use primitives::{Filesystem, LogFormat};

/// Source for pict-rs configuration when embedding as a library
pub enum ConfigSource<P, T> {
    /// A File source for pict-rs configuration
    File { path: P },
    /// An in-memory source for pict-rs configuration
    Memory { values: T },
    /// No configuration
    Empty,
}

impl<T> ConfigSource<PathBuf, T>
where
    T: serde::Serialize,
{
    /// Create a new memory source
    pub fn memory(values: T) -> Self {
        Self::Memory { values }
    }
}

impl<P> ConfigSource<P, ()>
where
    P: AsRef<Path>,
{
    /// Create a new file source
    pub fn file(path: P) -> Self {
        Self::File { path }
    }
}

impl ConfigSource<PathBuf, ()> {
    /// Create a new empty source
    pub fn empty() -> Self {
        Self::Empty
    }
}

pub struct PictRsConfiguration {
    pub(crate) config: Configuration,
    pub(crate) operation: Operation,
}

pub(crate) fn configure_without_clap<P: AsRef<Path>, T: serde::Serialize, Q: AsRef<Path>>(
    source: ConfigSource<P, T>,
    save_to: Option<Q>,
) -> color_eyre::Result<PictRsConfiguration> {
    let config = Config::builder().add_source(config::Config::try_from(&Defaults::default())?);

    let config = match source {
        ConfigSource::Empty => config,
        ConfigSource::File { path } => config.add_source(config::File::from(path.as_ref())),
        ConfigSource::Memory { values } => config.add_source(config::Config::try_from(&values)?),
    };

    let built = config
        .add_source(
            config::Environment::with_prefix("PICTRS")
                .separator("__")
                .try_parsing(true),
        )
        .build()?;

    let operation = Operation::Run;

    let config: Configuration = built.try_deserialize()?;

    if let Some(save_to) = save_to {
        let output = toml::to_string_pretty(&config)?;
        std::fs::write(save_to, output)?;
    }

    Ok(PictRsConfiguration { config, operation })
}

pub(crate) fn configure() -> color_eyre::Result<PictRsConfiguration> {
    let Output {
        config_format,
        operation,
        save_to,
        config_file,
    } = Args::parse().into_output();

    let config = Config::builder().add_source(config::Config::try_from(&Defaults::default())?);

    let config = if let Some(config_file) = config_file {
        config.add_source(config::File::from(config_file))
    } else {
        config
    };

    let built = config
        .add_source(
            config::Environment::with_prefix("PICTRS")
                .separator("__")
                .try_parsing(true),
        )
        .add_source(config::Config::try_from(&config_format)?)
        .build()?;

    let config: Configuration = built.try_deserialize()?;

    if let Some(save_to) = save_to {
        let output = toml::to_string_pretty(&config)?;
        std::fs::write(save_to, output)?;
    }

    Ok(PictRsConfiguration { config, operation })
}
