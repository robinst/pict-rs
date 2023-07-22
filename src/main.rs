#[actix_rt::main]
async fn main() -> color_eyre::Result<()> {
    pict_rs::PictRsConfiguration::build_default()?
        .install_tracing()?
        .run()
        .await
}
