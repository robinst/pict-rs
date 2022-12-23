#[actix_rt::main]
async fn main() -> color_eyre::Result<()> {
    pict_rs::install_tracing()?;

    pict_rs::run().await
}
