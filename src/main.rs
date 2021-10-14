use actix_form_data::{Field, Form, Value};
use actix_web::{
    guard,
    http::header::{CacheControl, CacheDirective, LastModified, ACCEPT_RANGES},
    web, App, HttpResponse, HttpResponseBuilder, HttpServer,
};
use awc::Client;
use dashmap::{mapref::entry::Entry, DashMap};
use futures_util::{stream::once, Stream};
use once_cell::sync::Lazy;
use opentelemetry::{
    sdk::{propagation::TraceContextPropagator, Resource},
    KeyValue,
};
use opentelemetry_otlp::WithExportConfig;
use std::{
    collections::HashSet,
    future::{ready, Future},
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
    time::SystemTime,
};
use structopt::StructOpt;
use tokio::{
    io::AsyncReadExt,
    sync::{
        oneshot::{Receiver, Sender},
        Semaphore,
    },
};
use tracing::{debug, error, info, instrument, subscriber::set_global_default, Span};
use tracing_actix_web::TracingLogger;
use tracing_awc::Propagate;
use tracing_error::ErrorLayer;
use tracing_futures::Instrument;
use tracing_log::LogTracer;
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, EnvFilter, Registry};

mod config;
mod either;
mod error;
mod exiftool;
mod ffmpeg;
mod file;
mod magick;
mod middleware;
mod migrate;
mod processor;
mod range;
mod stream;
mod upload_manager;
mod validate;

use self::{
    config::{Config, Format},
    either::Either,
    error::{Error, UploadError},
    middleware::{Deadline, Internal},
    upload_manager::{Details, UploadManager, UploadManagerSession},
    validate::{image_webp, video_mp4},
};

const MEGABYTES: usize = 1024 * 1024;
const MINUTES: u32 = 60;
const HOURS: u32 = 60 * MINUTES;
const DAYS: u32 = 24 * HOURS;

static TMP_DIR: Lazy<PathBuf> = Lazy::new(|| {
    use rand::{
        distributions::{Alphanumeric, Distribution},
        thread_rng,
    };

    let mut rng = thread_rng();
    let tmp_nonce = Alphanumeric
        .sample_iter(&mut rng)
        .take(7)
        .map(char::from)
        .collect::<String>();

    let mut path = std::env::temp_dir();
    path.push(format!("pict-rs-{}", tmp_nonce));
    path
});
static CONFIG: Lazy<Config> = Lazy::new(Config::from_args);
static PROCESS_SEMAPHORE: Lazy<Semaphore> =
    Lazy::new(|| Semaphore::new(num_cpus::get().saturating_sub(1).max(1)));
static PROCESS_MAP: Lazy<ProcessMap> = Lazy::new(DashMap::new);

type OutcomeSender = Sender<(Details, web::Bytes)>;
type ProcessMap = DashMap<PathBuf, Vec<OutcomeSender>>;

struct CancelSafeProcessor<F> {
    span: Span,
    path: PathBuf,
    receiver: Option<Receiver<(Details, web::Bytes)>>,
    fut: F,
}

impl<F> CancelSafeProcessor<F>
where
    F: Future<Output = Result<(Details, web::Bytes), Error>> + Unpin,
{
    pub(crate) fn new(path: PathBuf, fut: F) -> Self {
        let entry = PROCESS_MAP.entry(path.clone());

        let (receiver, span) = match entry {
            Entry::Vacant(vacant) => {
                vacant.insert(Vec::new());
                let span = tracing::info_span!(
                    "Processing image",
                    path = &tracing::field::debug(&path),
                    completed = &tracing::field::Empty,
                );
                (None, span)
            }
            Entry::Occupied(mut occupied) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                occupied.get_mut().push(tx);
                let span = tracing::info_span!(
                    "Waiting for processed image",
                    path = &tracing::field::debug(&path),
                );
                (Some(rx), span)
            }
        };

        CancelSafeProcessor {
            span,
            path,
            receiver,
            fut,
        }
    }
}

impl<F> Future for CancelSafeProcessor<F>
where
    F: Future<Output = Result<(Details, web::Bytes), Error>> + Unpin,
{
    type Output = Result<(Details, web::Bytes), Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let span = self.span.clone();

        span.in_scope(|| {
            if let Some(ref mut rx) = self.receiver {
                Pin::new(rx)
                    .poll(cx)
                    .map(|res| res.map_err(|_| UploadError::Canceled.into()))
            } else {
                Pin::new(&mut self.fut).poll(cx).map(|res| {
                    let opt = PROCESS_MAP.remove(&self.path);
                    res.map(|tup| {
                        if let Some((_, vec)) = opt {
                            for sender in vec {
                                let _ = sender.send(tup.clone());
                            }
                        }
                        tup
                    })
                })
            }
        })
    }
}

impl<F> Drop for CancelSafeProcessor<F> {
    fn drop(&mut self) {
        if self.receiver.is_none() {
            let completed = PROCESS_MAP.remove(&self.path).is_none();
            self.span.record("completed", &completed);
        }
    }
}

// try moving a file
#[instrument(name = "Moving file")]
async fn safe_move_file(from: PathBuf, to: PathBuf) -> Result<(), Error> {
    if let Some(path) = to.parent() {
        debug!("Creating directory {:?}", path);
        tokio::fs::create_dir_all(path).await?;
    }

    debug!("Checking if {:?} already exists", to);
    if let Err(e) = tokio::fs::metadata(&to).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    } else {
        return Err(UploadError::FileExists.into());
    }

    debug!("Moving {:?} to {:?}", from, to);
    tokio::fs::copy(&from, to).await?;
    tokio::fs::remove_file(from).await?;
    Ok(())
}

async fn safe_create_parent<P>(path: P) -> Result<(), Error>
where
    P: AsRef<std::path::Path>,
{
    if let Some(path) = path.as_ref().parent() {
        debug!("Creating directory {:?}", path);
        tokio::fs::create_dir_all(path).await?;
    }

    Ok(())
}

// Try writing to a file
#[instrument(name = "Saving file", skip(bytes))]
async fn safe_save_file(path: PathBuf, bytes: web::Bytes) -> Result<(), Error> {
    if let Some(path) = path.parent() {
        // create the directory for the file
        debug!("Creating directory {:?}", path);
        tokio::fs::create_dir_all(path).await?;
    }

    // Only write the file if it doesn't already exist
    debug!("Checking if {:?} already exists", path);
    if let Err(e) = tokio::fs::metadata(&path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    } else {
        return Ok(());
    }

    // Open the file for writing
    debug!("Creating {:?}", path);
    let mut file = crate::file::File::create(&path).await?;

    // try writing
    debug!("Writing to {:?}", path);
    if let Err(e) = file.write_from_bytes(bytes).await {
        error!("Error writing {:?}, {}", path, e);
        // remove file if writing failed before completion
        tokio::fs::remove_file(path).await?;
        return Err(e.into());
    }
    debug!("{:?} written", path);

    Ok(())
}

pub(crate) fn tmp_file() -> PathBuf {
    use rand::distributions::{Alphanumeric, Distribution};
    let limit: usize = 10;
    let rng = rand::thread_rng();

    let s: String = Alphanumeric
        .sample_iter(rng)
        .take(limit)
        .map(char::from)
        .collect();

    let name = format!("{}.tmp", s);

    let mut path = TMP_DIR.clone();
    path.push(&name);

    path
}

fn to_ext(mime: mime::Mime) -> Result<&'static str, Error> {
    if mime == mime::IMAGE_PNG {
        Ok(".png")
    } else if mime == mime::IMAGE_JPEG {
        Ok(".jpg")
    } else if mime == video_mp4() {
        Ok(".mp4")
    } else if mime == image_webp() {
        Ok(".webp")
    } else {
        Err(UploadError::UnsupportedFormat.into())
    }
}

/// Handle responding to succesful uploads
#[instrument(name = "Uploaded files", skip(value, manager))]
async fn upload(
    value: Value<UploadManagerSession>,
    manager: web::Data<UploadManager>,
) -> Result<HttpResponse, Error> {
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
            let mut path = manager.image_dir();
            path.push(name.clone());

            let details = manager.variant_details(path.clone(), name.clone()).await?;

            let details = if let Some(details) = details {
                debug!("details exist");
                details
            } else {
                debug!("generating new details from {:?}", path);
                let new_details = Details::from_path(path.clone()).await?;
                debug!("storing details for {:?} {}", path, name);
                manager
                    .store_variant_details(path, name, &new_details)
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

/// download an image from a URL
#[instrument(name = "Downloading file", skip(client, manager))]
async fn download(
    client: web::Data<Client>,
    manager: web::Data<UploadManager>,
    query: web::Query<UrlQuery>,
) -> Result<HttpResponse, Error> {
    let mut res = client.get(&query.url).propagate().send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()).into());
    }

    let fut = res.body().limit(CONFIG.max_file_size() * MEGABYTES);

    let stream = Box::pin(once(fut));

    let permit = PROCESS_SEMAPHORE.acquire().await?;
    let session = manager.session().upload(stream).await?;
    let alias = session.alias().unwrap().to_owned();
    drop(permit);
    let delete_token = session.delete_token().await?;

    let name = manager.from_alias(alias.to_owned()).await?;
    let mut path = manager.image_dir();
    path.push(name.clone());

    let details = manager.variant_details(path.clone(), name.clone()).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let new_details = Details::from_path(path.clone()).await?;
        manager
            .store_variant_details(path, name, &new_details)
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
async fn delete(
    manager: web::Data<UploadManager>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, Error> {
    let (alias, token) = path_entries.into_inner();

    manager.delete(token, alias).await?;

    Ok(HttpResponse::NoContent().finish())
}

type ProcessQuery = Vec<(String, String)>;

async fn prepare_process(
    query: web::Query<ProcessQuery>,
    ext: &str,
    manager: &UploadManager,
    whitelist: &Option<HashSet<String>>,
) -> Result<(Format, String, PathBuf, Vec<String>), Error> {
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

    let operations = if let Some(whitelist) = whitelist.as_ref() {
        operations
            .into_iter()
            .filter(|(k, _)| whitelist.contains(&k.to_lowercase()))
            .collect()
    } else {
        operations
    };

    let chain = self::processor::build_chain(&operations);

    let format = ext
        .parse::<Format>()
        .map_err(|_| UploadError::UnsupportedFormat)?;
    let processed_name = format!("{}.{}", name, ext);
    let base = manager.image_dir();
    let thumbnail_path = self::processor::build_path(base, &chain, processed_name);
    let thumbnail_args = self::processor::build_args(&chain);

    Ok((format, name, thumbnail_path, thumbnail_args))
}

#[instrument(name = "Fetching derived details", skip(manager, whitelist))]
async fn process_details(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager>,
    whitelist: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, Error> {
    let (_, name, thumbnail_path, _) =
        prepare_process(query, ext.as_str(), &manager, &whitelist).await?;

    let details = manager.variant_details(thumbnail_path, name).await?;

    let details = details.ok_or(UploadError::NoFiles)?;

    Ok(HttpResponse::Ok().json(&details))
}

/// Process files
#[instrument(name = "Serving processed image", skip(manager, whitelist))]
async fn process(
    range: Option<range::RangeHeader>,
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager>,
    whitelist: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, Error> {
    let (format, name, thumbnail_path, thumbnail_args) =
        prepare_process(query, ext.as_str(), &manager, &whitelist).await?;

    // If the thumbnail doesn't exist, we need to create it
    let thumbnail_exists = if let Err(e) = tokio::fs::metadata(&thumbnail_path)
        .instrument(tracing::info_span!("Get thumbnail metadata"))
        .await
    {
        if e.kind() != std::io::ErrorKind::NotFound {
            error!("Error looking up processed image, {}", e);
            return Err(e.into());
        }
        false
    } else {
        true
    };

    let details = manager
        .variant_details(thumbnail_path.clone(), name.clone())
        .await?;

    if !thumbnail_exists || details.is_none() {
        let mut original_path = manager.image_dir();
        original_path.push(name.clone());

        let thumbnail_path2 = thumbnail_path.clone();
        let process_fut = async {
            let thumbnail_path = thumbnail_path2;
            // Create and save a JPG for motion images (gif, mp4)
            if let Some((updated_path, exists)) =
                self::processor::prepare_image(original_path.clone()).await?
            {
                original_path = updated_path.clone();

                if exists.is_new() {
                    // Save the transcoded file in another task
                    debug!("Spawning storage task");
                    let manager2 = manager.clone();
                    let name = name.clone();
                    let span = tracing::info_span!(
                        parent: None,
                        "Storing variant info",
                        path = &tracing::field::debug(&updated_path),
                        name = &tracing::field::display(&name),
                    );
                    span.follows_from(Span::current());
                    actix_rt::spawn(
                        async move {
                            if let Err(e) = manager2.store_variant(updated_path, name).await {
                                error!("Error storing variant, {}", e);
                                return;
                            }
                        }
                        .instrument(span),
                    );
                }
            }

            let permit = PROCESS_SEMAPHORE.acquire().await?;

            let file = crate::file::File::open(original_path.clone()).await?;

            let mut processed_reader =
                crate::magick::process_image_file_read(file, thumbnail_args, format)?;

            let mut vec = Vec::new();
            processed_reader.read_to_end(&mut vec).await?;
            let bytes = web::Bytes::from(vec);

            drop(permit);

            let details = if let Some(details) = details {
                details
            } else {
                Details::from_bytes(bytes.clone()).await?
            };

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
                    if let Err(e) = safe_save_file(thumbnail_path.clone(), bytes2).await {
                        tracing::warn!("Error saving thumbnail: {}", e);
                        return;
                    }
                    if let Err(e) = manager
                        .store_variant_details(thumbnail_path.clone(), name.clone(), &details2)
                        .await
                    {
                        tracing::warn!("Error saving variant details: {}", e);
                        return;
                    }
                    if let Err(e) = manager.store_variant(thumbnail_path, name.clone()).await {
                        tracing::warn!("Error saving variant info: {}", e);
                    }
                }
                .instrument(save_span),
            );

            Ok((details, bytes)) as Result<(Details, web::Bytes), Error>
        };

        let (details, bytes) =
            CancelSafeProcessor::new(thumbnail_path.clone(), Box::pin(process_fut)).await?;

        return match range {
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
        };
    }

    let details = if let Some(details) = details {
        details
    } else {
        let details = Details::from_path(thumbnail_path.clone()).await?;
        manager
            .store_variant_details(thumbnail_path.clone(), name, &details)
            .await?;
        details
    };

    ranged_file_resp(thumbnail_path, range, details).await
}

/// Fetch file details
#[instrument(name = "Fetching details", skip(manager))]
async fn details(
    alias: web::Path<String>,
    manager: web::Data<UploadManager>,
) -> Result<HttpResponse, Error> {
    let name = manager.from_alias(alias.into_inner()).await?;
    let mut path = manager.image_dir();
    path.push(name.clone());

    let details = manager.variant_details(path.clone(), name.clone()).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let new_details = Details::from_path(path.clone()).await?;
        manager
            .store_variant_details(path.clone(), name, &new_details)
            .await?;
        new_details
    };

    Ok(HttpResponse::Ok().json(&details))
}

/// Serve files
#[instrument(name = "Serving file", skip(manager))]
async fn serve(
    range: Option<range::RangeHeader>,
    alias: web::Path<String>,
    manager: web::Data<UploadManager>,
) -> Result<HttpResponse, Error> {
    let name = manager.from_alias(alias.into_inner()).await?;
    let mut path = manager.image_dir();
    path.push(name.clone());

    let details = manager.variant_details(path.clone(), name.clone()).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let details = Details::from_path(path.clone()).await?;
        manager
            .store_variant_details(path.clone(), name, &details)
            .await?;
        details
    };

    ranged_file_resp(path, range, details).await
}

async fn ranged_file_resp(
    path: PathBuf,
    range: Option<range::RangeHeader>,
    details: Details,
) -> Result<HttpResponse, Error> {
    let (builder, stream) = match range {
        //Range header exists - return as ranged
        Some(range_header) => {
            if !range_header.is_bytes() {
                return Err(UploadError::Range.into());
            }

            if range_header.is_empty() {
                return Err(UploadError::Range.into());
            } else if range_header.len() == 1 {
                let file = crate::file::File::open(path).await?;

                let meta = file.metadata().await?;

                let range = range_header.ranges().next().unwrap();

                let mut builder = HttpResponse::PartialContent();
                builder.insert_header(range.to_content_range(meta.len()));

                (builder, Either::Left(range.chop_file(file).await?))
            } else {
                return Err(UploadError::Range.into());
            }
        }
        //No Range header in the request - return the entire document
        None => {
            let file = crate::file::File::open(path).await?;
            let stream = file.read_to_stream(None, None).await?;
            (HttpResponse::Ok(), Either::Right(stream))
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
    S: Stream<Item = Result<web::Bytes, E>> + Unpin + 'static,
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
        .streaming(stream)
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum FileOrAlias {
    File { file: String },
    Alias { alias: String },
}

#[instrument(name = "Purging file", skip(upload_manager))]
async fn purge(
    query: web::Query<FileOrAlias>,
    upload_manager: web::Data<UploadManager>,
) -> Result<HttpResponse, Error> {
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
async fn aliases(
    query: web::Query<FileOrAlias>,
    upload_manager: web::Data<UploadManager>,
) -> Result<HttpResponse, Error> {
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
async fn filename_by_alias(
    query: web::Query<ByAlias>,
    upload_manager: web::Data<UploadManager>,
) -> Result<HttpResponse, Error> {
    let filename = upload_manager.from_alias(query.into_inner().alias).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "filename": filename,
    })))
}

#[actix_rt::main]
async fn main() -> Result<(), anyhow::Error> {
    LogTracer::init()?;

    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let format_layer = tracing_subscriber::fmt::layer()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .pretty();

    let subscriber = Registry::default()
        .with(env_filter)
        .with(format_layer)
        .with(ErrorLayer::default());

    if let Some(url) = CONFIG.opentelemetry_url() {
        let tracer =
            opentelemetry_otlp::new_pipeline()
                .tracing()
                .with_trace_config(opentelemetry::sdk::trace::config().with_resource(
                    Resource::new(vec![KeyValue::new("service.name", "pict-rs")]),
                ))
                .with_exporter(
                    opentelemetry_otlp::new_exporter()
                        .tonic()
                        .with_endpoint(url.as_str()),
                )
                .install_batch(opentelemetry::runtime::Tokio)?;

        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        let subscriber = subscriber.with(otel_layer);

        set_global_default(subscriber)?;
    } else {
        let subscriber = subscriber.with(tracing_subscriber::fmt::layer());
        set_global_default(subscriber)?;
    };

    let manager = UploadManager::new(CONFIG.data_dir(), CONFIG.format()).await?;

    // Create a new Multipart Form validator
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let manager2 = manager.clone();
    let form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.max_file_size() * MEGABYTES)
        .transform_error(|e| Error::from(e).into())
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let manager = manager2.clone();

                let span = tracing::info_span!("file-upload", ?filename);

                async move {
                    let permit = PROCESS_SEMAPHORE.acquire().await?;

                    let res = manager.session().upload(stream).await;

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
        .transform_error(|e| Error::from(e).into())
        .field(
            "images",
            Field::array(Field::file(move |filename, content_type, stream| {
                let manager = manager2.clone();

                let span = tracing::info_span!("file-import", ?filename);

                async move {
                    let permit = PROCESS_SEMAPHORE.acquire().await?;

                    let res = manager
                        .session()
                        .import(filename, content_type, validate_imports, stream)
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
            .app_data(web::Data::new(CONFIG.filter_whitelist()))
            .service(
                web::scope("/image")
                    .service(
                        web::resource("")
                            .guard(guard::Post())
                            .wrap(form.clone())
                            .route(web::post().to(upload)),
                    )
                    .service(web::resource("/download").route(web::get().to(download)))
                    .service(
                        web::resource("/delete/{delete_token}/{filename}")
                            .route(web::delete().to(delete))
                            .route(web::get().to(delete)),
                    )
                    .service(web::resource("/original/{filename}").route(web::get().to(serve)))
                    .service(web::resource("/process.{ext}").route(web::get().to(process)))
                    .service(
                        web::scope("/details")
                            .service(
                                web::resource("/original/{filename}").route(web::get().to(details)),
                            )
                            .service(
                                web::resource("/process.{ext}")
                                    .route(web::get().to(process_details)),
                            ),
                    ),
            )
            .service(
                web::scope("/internal")
                    .wrap(Internal(CONFIG.api_key().map(|s| s.to_owned())))
                    .service(
                        web::resource("/import")
                            .wrap(import_form.clone())
                            .route(web::post().to(upload)),
                    )
                    .service(web::resource("/purge").route(web::post().to(purge)))
                    .service(web::resource("/aliases").route(web::get().to(aliases)))
                    .service(web::resource("/filename").route(web::get().to(filename_by_alias))),
            )
    })
    .bind(CONFIG.bind_address())?
    .run()
    .await?;

    if tokio::fs::metadata(&*TMP_DIR).await.is_ok() {
        tokio::fs::remove_dir_all(&*TMP_DIR).await?;
    }

    Ok(())
}
