fn main() -> color_eyre::Result<()> {
    run()
}

#[cfg(feature = "io-uring")]
fn run() -> color_eyre::Result<()> {
    tokio_uring::start(async move {
        pict_rs::PictRsConfiguration::build_default()?
            .install_tracing()?
            .install_metrics()?
            .run()
            .await
    })
}

#[cfg(not(feature = "io-uring"))]
fn run() -> color_eyre::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            pict_rs::PictRsConfiguration::build_default()?
                .install_tracing()?
                .install_metrics()?
                .run_on_localset()
                .await
        })
}
