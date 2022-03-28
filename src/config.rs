use clap::Parser;

mod commandline;
mod defaults;
mod file;
mod primitives;

use commandline::{Args, Output};
use config::Config;
use defaults::Defaults;

pub(crate) use commandline::Operation;
pub(crate) use file::{ConfigFile as Configuration, OpenTelemetry, Repo, Sled, Tracing};
pub(crate) use primitives::{Filesystem, ImageFormat, LogFormat, ObjectStorage, Store};

pub(crate) fn configure() -> anyhow::Result<(Configuration, Operation)> {
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
