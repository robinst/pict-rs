use actix_form_data::{Field, Form, Value};
use actix_web::{
    guard,
    http::header::{CacheControl, CacheDirective, LastModified, ACCEPT_RANGES},
    web, App, HttpResponse, HttpResponseBuilder, HttpServer,
};
use awc::Client;
use futures_util::{stream::once, Stream};
use once_cell::sync::Lazy;
use std::{
    collections::HashSet,
    future::ready,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
    time::SystemTime,
};
use structopt::StructOpt;
use tokio::{io::AsyncReadExt, sync::Semaphore};
use tracing::{debug, error, info, instrument, Span};
use tracing_actix_web::TracingLogger;
use tracing_awc::Propagate;
use tracing_futures::Instrument;

mod concurrent_processor;
mod config;
mod either;
mod error;
mod exiftool;
mod ffmpeg;
mod init_tracing;
mod magick;
mod map_error;
mod middleware;
mod migrate;
mod process;
mod processor;
mod range;
mod store;
mod upload_manager;
mod validate;

use crate::{magick::details_hint, store::file_store::FileStore};

use self::{
    concurrent_processor::CancelSafeProcessor,
    config::{Config, Format},
    either::Either,
    error::{Error, UploadError},
    init_tracing::init_tracing,
    middleware::{Deadline, Internal},
    migrate::LatestDb,
    store::Store,
    upload_manager::{Details, UploadManager, UploadManagerSession},
};

const MEGABYTES: usize = 1024 * 1024;
const MINUTES: u32 = 60;
const HOURS: u32 = 60 * MINUTES;
const DAYS: u32 = 24 * HOURS;

static CONFIG: Lazy<Config> = Lazy::new(Config::from_args);
static PROCESS_SEMAPHORE: Lazy<Semaphore> =
    Lazy::new(|| Semaphore::new(num_cpus::get().saturating_sub(1).max(1)));

/// Handle responding to succesful uploads
#[instrument(name = "Uploaded files", skip(value, manager))]
async fn upload<S: Store>(
    value: Value<UploadManagerSession<S>>,
    manager: web::Data<UploadManager<S>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let images = value
        .map()
        .and_then(|mut m| m.remove("images"))
        .and_then(|images| images.array())
        .ok_or(UploadError::NoFiles)?;

    let mut files = Vec::new();
    let images = images
        .into_iter()
        .filter_map(|i| i.file())
        .collect::<Vec<_>>();
    for image in &images {
        if let Some(alias) = image.result.alias() {
            info!("Uploaded {} as {:?}", image.filename, alias);
            let delete_token = image.result.delete_token().await?;

            let name = manager.from_alias(alias.to_owned()).await?;
            let identifier = manager.identifier_from_filename(name.clone()).await?;

            let details = manager
                .variant_details(identifier.clone(), name.clone())
                .await?;

            let details = if let Some(details) = details {
                debug!("details exist");
                details
            } else {
                debug!("generating new details from {:?}", identifier);
                let hint = details_hint(&name);
                let new_details =
                    Details::from_store(manager.store().clone(), identifier.clone(), hint).await?;
                debug!("storing details for {:?} {}", identifier, name);
                manager
                    .store_variant_details(identifier, name, &new_details)
                    .await?;
                debug!("stored");
                new_details
            };

            files.push(serde_json::json!({
                "file": alias,
                "delete_token": delete_token,
                "details": details,
            }));
        }
    }

    for image in images {
        image.result.succeed();
    }
    Ok(HttpResponse::Created().json(&serde_json::json!({
        "msg": "ok",
        "files": files
    })))
}

#[derive(Debug, serde::Deserialize)]
struct UrlQuery {
    url: String,
}

pin_project_lite::pin_project! {
    struct Limit<S> {
        #[pin]
        inner: S,

        count: u64,
        limit: u64,
    }
}

impl<S> Limit<S> {
    fn new(inner: S, limit: u64) -> Self {
        Limit {
            inner,
            count: 0,
            limit,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Resonse body larger than size limit")]
struct LimitError;

impl<S, E> Stream for Limit<S>
where
    S: Stream<Item = Result<web::Bytes, E>>,
    E: From<LimitError>,
{
    type Item = Result<web::Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().project();

        let limit = this.limit;
        let count = this.count;
        let inner = this.inner;

        inner.poll_next(cx).map(|opt| {
            opt.map(|res| match res {
                Ok(bytes) => {
                    *count += bytes.len() as u64;
                    if *count > *limit {
                        return Err(LimitError.into());
                    }
                    Ok(bytes)
                }
                Err(e) => Err(e),
            })
        })
    }
}

/// download an image from a URL
#[instrument(name = "Downloading file", skip(client, manager))]
async fn download<S: Store>(
    client: web::Data<Client>,
    manager: web::Data<UploadManager<S>>,
    query: web::Query<UrlQuery>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let res = client.get(&query.url).propagate().send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()).into());
    }

    let mut stream = Limit::new(
        map_error::map_crate_error(res),
        (CONFIG.max_file_size() * MEGABYTES) as u64,
    );

    // SAFETY: stream is shadowed, so original cannot not be moved
    let stream = unsafe { Pin::new_unchecked(&mut stream) };

    let permit = PROCESS_SEMAPHORE.acquire().await?;
    let session = manager.session().upload(stream).await?;
    let alias = session.alias().unwrap().to_owned();
    drop(permit);
    let delete_token = session.delete_token().await?;

    let name = manager.from_alias(alias.to_owned()).await?;
    let identifier = manager.identifier_from_filename(name.clone()).await?;

    let details = manager
        .variant_details(identifier.clone(), name.clone())
        .await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&name);
        let new_details =
            Details::from_store(manager.store().clone(), identifier.clone(), hint).await?;
        manager
            .store_variant_details(identifier, name, &new_details)
            .await?;
        new_details
    };

    session.succeed();
    Ok(HttpResponse::Created().json(&serde_json::json!({
        "msg": "ok",
        "files": [{
            "file": alias,
            "delete_token": delete_token,
            "details": details,
        }]
    })))
}

/// Delete aliases and files
#[instrument(name = "Deleting file", skip(manager))]
async fn delete<S: Store>(
    manager: web::Data<UploadManager<S>>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let (alias, token) = path_entries.into_inner();

    manager.delete(token, alias).await?;

    Ok(HttpResponse::NoContent().finish())
}

type ProcessQuery = Vec<(String, String)>;

async fn prepare_process<S: Store>(
    query: web::Query<ProcessQuery>,
    ext: &str,
    manager: &UploadManager<S>,
    filters: &Option<HashSet<String>>,
) -> Result<(Format, String, PathBuf, Vec<String>), Error>
where
    Error: From<S::Error>,
{
    let (alias, operations) =
        query
            .into_inner()
            .into_iter()
            .fold((String::new(), Vec::new()), |(s, mut acc), (k, v)| {
                if k == "src" {
                    (v, acc)
                } else {
                    acc.push((k, v));
                    (s, acc)
                }
            });

    if alias.is_empty() {
        return Err(UploadError::MissingFilename.into());
    }

    let name = manager.from_alias(alias).await?;

    let operations = if let Some(filters) = filters.as_ref() {
        operations
            .into_iter()
            .filter(|(k, _)| filters.contains(&k.to_lowercase()))
            .collect()
    } else {
        operations
    };

    let format = ext
        .parse::<Format>()
        .map_err(|_| UploadError::UnsupportedFormat)?;
    let processed_name = format!("{}.{}", name, ext);

    let (thumbnail_path, thumbnail_args) =
        self::processor::build_chain(&operations, processed_name)?;

    Ok((format, name, thumbnail_path, thumbnail_args))
}

#[instrument(name = "Fetching derived details", skip(manager, filters))]
async fn process_details<S: Store>(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager<S>>,
    filters: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let (_, name, thumbnail_path, _) =
        prepare_process(query, ext.as_str(), &manager, &filters).await?;

    let identifier = manager
        .variant_identifier(&thumbnail_path, &name)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    let details = manager.variant_details(identifier, name).await?;

    let details = details.ok_or(UploadError::NoFiles)?;

    Ok(HttpResponse::Ok().json(&details))
}

/// Process files
#[instrument(name = "Serving processed image", skip(manager, filters))]
async fn process<S: Store + 'static>(
    range: Option<range::RangeHeader>,
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager<S>>,
    filters: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let (format, name, thumbnail_path, thumbnail_args) =
        prepare_process(query, ext.as_str(), &manager, &filters).await?;

    let identifier_opt = manager.variant_identifier(&thumbnail_path, &name).await?;

    if let Some(identifier) = identifier_opt {
        let details_opt = manager
            .variant_details(identifier.clone(), name.clone())
            .await?;

        let details = if let Some(details) = details_opt {
            details
        } else {
            let hint = details_hint(&name);
            let details =
                Details::from_store(manager.store().clone(), identifier.clone(), hint).await?;
            manager
                .store_variant_details(identifier.clone(), name, &details)
                .await?;
            details
        };

        return ranged_file_resp(manager.store().clone(), identifier, range, details).await;
    }

    let identifier = manager.still_identifier_from_filename(name.clone()).await?;

    let thumbnail_path2 = thumbnail_path.clone();
    let process_fut = async {
        let thumbnail_path = thumbnail_path2;

        let permit = PROCESS_SEMAPHORE.acquire().await?;

        let mut processed_reader = crate::magick::process_image_store_read(
            manager.store().clone(),
            identifier,
            thumbnail_args,
            format,
        )?;

        let mut vec = Vec::new();
        processed_reader.read_to_end(&mut vec).await?;
        let bytes = web::Bytes::from(vec);

        drop(permit);

        let details = Details::from_bytes(bytes.clone(), format.to_hint()).await?;

        let save_span = tracing::info_span!(
            parent: None,
            "Saving variant information",
            path = tracing::field::debug(&thumbnail_path),
            name = tracing::field::display(&name),
        );
        save_span.follows_from(Span::current());
        let details2 = details.clone();
        let bytes2 = bytes.clone();
        actix_rt::spawn(
            async move {
                let identifier = match manager.store().save_bytes(bytes2).await {
                    Ok(identifier) => identifier,
                    Err(e) => {
                        tracing::warn!("Failed to generate directory path: {}", e);
                        return;
                    }
                };
                if let Err(e) = manager
                    .store_variant_details(identifier.clone(), name.clone(), &details2)
                    .await
                {
                    tracing::warn!("Error saving variant details: {}", e);
                    return;
                }
                if let Err(e) = manager
                    .store_variant(Some(&thumbnail_path), &identifier, &name)
                    .await
                {
                    tracing::warn!("Error saving variant info: {}", e);
                }
            }
            .instrument(save_span),
        );

        Ok((details, bytes)) as Result<(Details, web::Bytes), Error>
    };

    let (details, bytes) = CancelSafeProcessor::new(thumbnail_path.clone(), process_fut).await?;

    match range {
        Some(range_header) => {
            if !range_header.is_bytes() {
                return Err(UploadError::Range.into());
            }

            if range_header.is_empty() {
                Err(UploadError::Range.into())
            } else if range_header.len() == 1 {
                let range = range_header.ranges().next().unwrap();
                let content_range = range.to_content_range(bytes.len() as u64);
                let stream = range.chop_bytes(bytes);
                let mut builder = HttpResponse::PartialContent();
                builder.insert_header(content_range);

                Ok(srv_response(
                    builder,
                    stream,
                    details.content_type(),
                    7 * DAYS,
                    details.system_time(),
                ))
            } else {
                Err(UploadError::Range.into())
            }
        }
        None => Ok(srv_response(
            HttpResponse::Ok(),
            once(ready(Ok(bytes) as Result<_, Error>)),
            details.content_type(),
            7 * DAYS,
            details.system_time(),
        )),
    }
}

/// Fetch file details
#[instrument(name = "Fetching details", skip(manager))]
async fn details<S: Store>(
    alias: web::Path<String>,
    manager: web::Data<UploadManager<S>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let name = manager.from_alias(alias.into_inner()).await?;
    let identifier = manager.identifier_from_filename(name.clone()).await?;

    let details = manager
        .variant_details(identifier.clone(), name.clone())
        .await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&name);
        let new_details =
            Details::from_store(manager.store().clone(), identifier.clone(), hint).await?;
        manager
            .store_variant_details(identifier, name, &new_details)
            .await?;
        new_details
    };

    Ok(HttpResponse::Ok().json(&details))
}

/// Serve files
#[instrument(name = "Serving file", skip(manager))]
async fn serve<S: Store>(
    range: Option<range::RangeHeader>,
    alias: web::Path<String>,
    manager: web::Data<UploadManager<S>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let name = manager.from_alias(alias.into_inner()).await?;
    let identifier = manager.identifier_from_filename(name.clone()).await?;

    let details = manager
        .variant_details(identifier.clone(), name.clone())
        .await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&name);
        let details =
            Details::from_store(manager.store().clone(), identifier.clone(), hint).await?;
        manager
            .store_variant_details(identifier.clone(), name, &details)
            .await?;
        details
    };

    ranged_file_resp(manager.store().clone(), identifier, range, details).await
}

async fn ranged_file_resp<S: Store>(
    store: S,
    identifier: S::Identifier,
    range: Option<range::RangeHeader>,
    details: Details,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let (builder, stream) = match range {
        //Range header exists - return as ranged
        Some(range_header) => {
            if !range_header.is_bytes() {
                return Err(UploadError::Range.into());
            }

            if range_header.is_empty() {
                return Err(UploadError::Range.into());
            } else if range_header.len() == 1 {
                let len = store.len(&identifier).await?;

                let range = range_header.ranges().next().unwrap();

                let mut builder = HttpResponse::PartialContent();
                builder.insert_header(range.to_content_range(len));

                (
                    builder,
                    Either::left(map_error::map_crate_error(
                        range.chop_store(store, identifier).await?,
                    )),
                )
            } else {
                return Err(UploadError::Range.into());
            }
        }
        //No Range header in the request - return the entire document
        None => {
            let stream =
                map_error::map_crate_error(store.to_stream(&identifier, None, None).await?);
            (HttpResponse::Ok(), Either::right(stream))
        }
    };

    Ok(srv_response(
        builder,
        stream,
        details.content_type(),
        7 * DAYS,
        details.system_time(),
    ))
}

// A helper method to produce responses with proper cache headers
fn srv_response<S, E>(
    mut builder: HttpResponseBuilder,
    stream: S,
    ext: mime::Mime,
    expires: u32,
    modified: SystemTime,
) -> HttpResponse
where
    S: Stream<Item = Result<web::Bytes, E>> + 'static,
    E: std::error::Error + 'static,
    actix_web::Error: From<E>,
{
    builder
        .insert_header(LastModified(modified.into()))
        .insert_header(CacheControl(vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(expires),
            CacheDirective::Extension("immutable".to_owned(), None),
        ]))
        .insert_header((ACCEPT_RANGES, "bytes"))
        .content_type(ext.to_string())
        // TODO: remove pin when actix-web drops Unpin requirement
        .streaming(Box::pin(stream))
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum FileOrAlias {
    File { file: String },
    Alias { alias: String },
}

#[instrument(name = "Purging file", skip(upload_manager))]
async fn purge<S: Store>(
    query: web::Query<FileOrAlias>,
    upload_manager: web::Data<UploadManager<S>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let aliases = match query.into_inner() {
        FileOrAlias::File { file } => upload_manager.aliases_by_filename(file).await?,
        FileOrAlias::Alias { alias } => upload_manager.aliases_by_alias(alias).await?,
    };

    for alias in aliases.iter() {
        upload_manager
            .delete_without_token(alias.to_owned())
            .await?;
    }

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases
    })))
}

#[instrument(name = "Fetching aliases", skip(upload_manager))]
async fn aliases<S: Store>(
    query: web::Query<FileOrAlias>,
    upload_manager: web::Data<UploadManager<S>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let aliases = match query.into_inner() {
        FileOrAlias::File { file } => upload_manager.aliases_by_filename(file).await?,
        FileOrAlias::Alias { alias } => upload_manager.aliases_by_alias(alias).await?,
    };

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct ByAlias {
    alias: String,
}

#[instrument(name = "Fetching filename", skip(upload_manager))]
async fn filename_by_alias<S: Store>(
    query: web::Query<ByAlias>,
    upload_manager: web::Data<UploadManager<S>>,
) -> Result<HttpResponse, Error>
where
    Error: From<S::Error>,
{
    let filename = upload_manager.from_alias(query.into_inner().alias).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "filename": filename,
    })))
}

fn transform_error(error: actix_form_data::Error) -> actix_web::Error {
    let error: Error = error.into();
    let error: actix_web::Error = error.into();
    error
}

async fn launch<S: Store>(manager: UploadManager<S>) -> anyhow::Result<()>
where
    S::Error: Unpin,
    Error: From<S::Error>,
{
    // Create a new Multipart Form validator
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let manager2 = manager.clone();
    let form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.max_file_size() * MEGABYTES)
        .transform_error(transform_error)
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let manager = manager2.clone();

                let span = tracing::info_span!("file-upload", ?filename);

                async move {
                    let permit = PROCESS_SEMAPHORE.acquire().await?;

                    let res = manager
                        .session()
                        .upload(map_error::map_crate_error(stream))
                        .await;

                    drop(permit);
                    res
                }
                .instrument(span)
            })),
        );

    // Create a new Multipart Form validator for internal imports
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let validate_imports = CONFIG.validate_imports();
    let manager2 = manager.clone();
    let import_form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.max_file_size() * MEGABYTES)
        .transform_error(transform_error)
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let manager = manager2.clone();

                let span = tracing::info_span!("file-import", ?filename);

                async move {
                    let permit = PROCESS_SEMAPHORE.acquire().await?;

                    let res = manager
                        .session()
                        .import(
                            filename,
                            validate_imports,
                            map_error::map_crate_error(stream),
                        )
                        .await;

                    drop(permit);
                    res
                }
                .instrument(span)
            })),
        );

    HttpServer::new(move || {
        let client = Client::builder()
            .header("User-Agent", "pict-rs v0.3.0-main")
            .finish();

        App::new()
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .app_data(web::Data::new(manager.clone()))
            .app_data(web::Data::new(client))
            .app_data(web::Data::new(CONFIG.allowed_filters()))
            .service(
                web::scope("/image")
                    .service(
                        web::resource("")
                            .guard(guard::Post())
                            .wrap(form.clone())
                            .route(web::post().to(upload::<S>)),
                    )
                    .service(web::resource("/download").route(web::get().to(download::<S>)))
                    .service(
                        web::resource("/delete/{delete_token}/{filename}")
                            .route(web::delete().to(delete::<S>))
                            .route(web::get().to(delete::<S>)),
                    )
                    .service(web::resource("/original/{filename}").route(web::get().to(serve::<S>)))
                    .service(web::resource("/process.{ext}").route(web::get().to(process::<S>)))
                    .service(
                        web::scope("/details")
                            .service(
                                web::resource("/original/{filename}")
                                    .route(web::get().to(details::<S>)),
                            )
                            .service(
                                web::resource("/process.{ext}")
                                    .route(web::get().to(process_details::<S>)),
                            ),
                    ),
            )
            .service(
                web::scope("/internal")
                    .wrap(Internal(CONFIG.api_key().map(|s| s.to_owned())))
                    .service(
                        web::resource("/import")
                            .wrap(import_form.clone())
                            .route(web::post().to(upload::<S>)),
                    )
                    .service(web::resource("/purge").route(web::post().to(purge::<S>)))
                    .service(web::resource("/aliases").route(web::get().to(aliases::<S>)))
                    .service(
                        web::resource("/filename").route(web::get().to(filename_by_alias::<S>)),
                    ),
            )
    })
    .bind(CONFIG.bind_address())?
    .run()
    .await?;

    Ok(())
}

#[actix_rt::main]
async fn main() -> anyhow::Result<()> {
    init_tracing("pict-rs", CONFIG.opentelemetry_url())?;

    let root_dir = CONFIG.data_dir();
    let db = LatestDb::exists(root_dir.clone()).migrate()?;

    let store = FileStore::build(root_dir, &db)?;

    let manager = UploadManager::new(store, db, CONFIG.format()).await?;

    manager.restructure().await?;
    launch(manager).await
}
