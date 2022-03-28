mod commandline;
mod defaults;
mod file;
mod primitives;

use crate::magick::ValidInputType;

pub(crate) use file::ConfigFile as Configuration;

pub(crate) fn configure() -> anyhow::Result<Configuration> {
    unimplemented!()
}
