pub(crate) fn clear_metadata_stream<S, E>(
    input: S,
) -> std::io::Result<futures::stream::LocalBoxStream<'static, Result<actix_web::web::Bytes, E>>>
where
    S: futures::stream::Stream<Item = Result<actix_web::web::Bytes, E>> + Unpin + 'static,
    E: From<std::io::Error> + 'static,
{
    let process = crate::stream::Process::spawn(
        tokio::process::Command::new("exiftool").args(["-all=", "-", "-out", "-"]),
    )?;

    Ok(Box::pin(process.sink_stream(input).unwrap()))
}
