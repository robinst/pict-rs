#[derive(Debug, thiserror::Error)]
pub(crate) enum Exvi2Error {
    #[error("Failed to interface with exiv2")]
    IO(#[from] std::io::Error),

    #[error("Identify semaphore is closed")]
    Closed,

    #[error("Exiv2 command failed")]
    Status,
}

static MAX_READS: once_cell::sync::OnceCell<tokio::sync::Semaphore> =
    once_cell::sync::OnceCell::new();

fn semaphore() -> &'static tokio::sync::Semaphore {
    MAX_READS.get_or_init(|| tokio::sync::Semaphore::new(num_cpus::get() * 5))
}

pub(crate) async fn clear_metadata<P>(file: P) -> Result<(), Exvi2Error>
where
    P: AsRef<std::path::Path>,
{
    let permit = semaphore().acquire().await?;

    let status = tokio::process::Command::new("exiv2")
        .arg(&"rm")
        .arg(&file.as_ref())
        .spawn()?
        .wait()
        .await?;

    drop(permit);

    if !status.success() {
        return Err(Exvi2Error::Status);
    }

    Ok(())
}

impl From<tokio::sync::AcquireError> for Exvi2Error {
    fn from(_: tokio::sync::AcquireError) -> Exvi2Error {
        Exvi2Error::Closed
    }
}
