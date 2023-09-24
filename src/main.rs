fn main() -> color_eyre::Result<()> {
    actix_web::rt::System::new().block_on(async move {
        pict_rs::PictRsConfiguration::build_default()?
            .install_tracing()?
            .install_metrics()?
            .run()
            .await
    })
}
