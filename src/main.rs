fn main() -> color_eyre::Result<()> {
    let config = pict_rs::PictRsConfiguration::build_default()?
        .install_tracing()?
        .install_metrics()?;

    run(config)
}

#[cfg(feature = "io-uring")]
fn run(config: pict_rs::PictRsConfiguration) -> color_eyre::Result<()> {
    tokio_uring::start(config.run())
}

#[cfg(not(feature = "io-uring"))]
fn run(config: pict_rs::PictRsConfiguration) -> color_eyre::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(config.run())
}
