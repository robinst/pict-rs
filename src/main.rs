#[actix_rt::main]
async fn main() -> color_eyre::Result<()> {
    let pict_rs = pict_rs::PictRsConfiguration::build_default()?;
    pict_rs.install_tracing()?;
    pict_rs.run().await
}
