#[cfg(feature = "io-uring")]
fn main() -> color_eyre::Result<()> {
    actix_web::rt::System::new().block_on(async move {
        pict_rs::PictRsConfiguration::build_default()?
            .install_tracing()?
            .install_metrics()?
            .install_crypto_provider()
            .run()
            .await
    })
}

#[cfg(not(feature = "io-uring"))]
fn main() -> color_eyre::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            pict_rs::PictRsConfiguration::build_default()?
                .install_tracing()?
                .install_metrics()?
                .install_crypto_provider()
                .run_on_localset()
                .await
        })
}
