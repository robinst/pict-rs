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
#[tokio::main]
async fn run() -> color_eyre::Result<()> {
    pict_rs::PictRsConfiguration::build_default()?
        .install_tracing()?
        .install_metrics()?
        .run_on_localset()
        .await
}
