fn main() -> color_eyre::Result<()> {
    #[cfg(not(feature = "io-uring"))]
    return run_tokio();

    #[cfg(feature = "io-uring")]
    return run_tokio_uring();
}

#[cfg(feature = "io-uring")]
fn run_tokio_uring() -> color_eyre::Result<()> {
    tokio_uring::start(run())
}

#[cfg(not(feature = "io-uring"))]
#[tokio::main]
async fn run_tokio() -> color_eyre::Result<()> {
    run().await
}

async fn run() -> color_eyre::Result<()> {
    pict_rs::PictRsConfiguration::build_default()?
        .install_tracing()?
        .install_metrics()?
        .run_on_localset()
        .await
}
