use clap::Parser;
use std::path::Path;

mod commandline;
mod defaults;
mod file;
mod primitives;

use commandline::{Args, Output};
use config::Config;
use defaults::Defaults;

pub(crate) use commandline::Operation;
pub(crate) use file::{ConfigFile as Configuration, OpenTelemetry, Repo, Sled, Tracing};
pub(crate) use primitives::{
    AudioCodec, Filesystem, ImageFormat, LogFormat, ObjectStorage, Store, VideoCodec,
};

pub(crate) fn configure_without_clap<P: AsRef<Path>, Q: AsRef<Path>>(
    config_file: Option<P>,
    save_to: Option<Q>,
) -> color_eyre::Result<(Configuration, Operation)> {
    let config = Config::builder().add_source(config::Config::try_from(&Defaults::default())?);

    let config = if let Some(config_file) = config_file {
        config.add_source(config::File::from(config_file.as_ref()))
    } else {
        config
    };

    let built = config
        .add_source(config::Environment::with_prefix("PICTRS").separator("__"))
        .build()?;

    let operation = Operation::Run;

    let config: Configuration = built.try_deserialize()?;

    if let Some(save_to) = save_to {
        let output = toml::to_string_pretty(&config)?;
        std::fs::write(save_to, output)?;
    }

    Ok((config, operation))
}

pub(crate) fn configure() -> color_eyre::Result<(Configuration, Operation)> {
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
        .add_source(config::Environment::with_prefix("PICTRS").separator("__"))
        .add_source(config::Config::try_from(&config_format)?)
        .build()?;

    let config: Configuration = built.try_deserialize()?;

    if let Some(save_to) = save_to {
        let output = toml::to_string_pretty(&config)?;
        std::fs::write(save_to, output)?;
    }

    Ok((config, operation))
}
