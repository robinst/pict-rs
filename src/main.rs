use actix_form_data::{Field, Form, Value};
use actix_web::{
    guard,
    http::header::{CacheControl, CacheDirective, LastModified, Range, ACCEPT_RANGES},
    web, App, HttpResponse, HttpResponseBuilder, HttpServer,
};
use awc::Client;
use futures_util::{
    stream::{empty, once},
    Stream, StreamExt, TryStreamExt,
};
use once_cell::sync::Lazy;
use std::{
    future::ready,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};
use tokio::sync::Semaphore;
use tracing::{debug, info, instrument};
use tracing_actix_web::TracingLogger;
use tracing_awc::Tracing;
use tracing_futures::Instrument;

mod backgrounded;
mod concurrent_processor;
mod config;
mod details;
mod either;
mod error;
mod exiftool;
mod ffmpeg;
mod file;
mod generate;
mod ingest;
mod init_tracing;
mod magick;
mod middleware;
mod process;
mod processor;
mod queue;
mod range;
mod repo;
mod serde_str;
mod store;
mod stream;
mod tmp_file;
mod validate;

use crate::repo::UploadResult;

use self::{
    backgrounded::Backgrounded,
    config::{Configuration, ImageFormat, Operation},
    details::Details,
    either::Either,
    error::{Error, UploadError},
    ingest::Session,
    init_tracing::init_tracing,
    magick::details_hint,
    middleware::{Deadline, Internal},
    queue::queue_generate,
    repo::{Alias, DeleteToken, FullRepo, HashRepo, IdentifierRepo, Repo, SettingsRepo, UploadId},
    serde_str::Serde,
    store::{file_store::FileStore, object_store::ObjectStore, Identifier, Store},
    stream::{StreamLimit, StreamTimeout},
};

const MEGABYTES: usize = 1024 * 1024;
const MINUTES: u32 = 60;
const HOURS: u32 = 60 * MINUTES;
const DAYS: u32 = 24 * HOURS;

static DO_CONFIG: Lazy<(Configuration, Operation)> =
    Lazy::new(|| config::configure().expect("Failed to configure"));
static CONFIG: Lazy<Configuration> = Lazy::new(|| DO_CONFIG.0.clone());
static OPERATION: Lazy<Operation> = Lazy::new(|| DO_CONFIG.1.clone());
static PROCESS_SEMAPHORE: Lazy<Semaphore> = Lazy::new(|| {
    tracing::trace_span!(parent: None, "Initialize semaphore")
        .in_scope(|| Semaphore::new(num_cpus::get().saturating_sub(1).max(1)))
});

/// Handle responding to succesful uploads
#[instrument(name = "Uploaded files", skip(value))]
async fn upload<R: FullRepo, S: Store + 'static>(
    value: Value<Session<R, S>>,
    repo: web::Data<R>,
    store: web::Data<S>,
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

            let identifier = repo.identifier_from_alias::<S::Identifier>(alias).await?;
            let details = repo.details(&identifier).await?;

            let details = if let Some(details) = details {
                debug!("details exist");
                details
            } else {
                debug!("generating new details from {:?}", identifier);
                let hint = details_hint(alias);
                let new_details =
                    Details::from_store((**store).clone(), identifier.clone(), hint).await?;
                debug!("storing details for {:?}", identifier);
                repo.relate_details(&identifier, &new_details).await?;
                debug!("stored");
                new_details
            };

            files.push(serde_json::json!({
                "file": alias.to_string(),
                "delete_token": delete_token.to_string(),
                "details": details,
            }));
        }
    }

    for mut image in images {
        image.result.disarm();
    }

    Ok(HttpResponse::Created().json(&serde_json::json!({
        "msg": "ok",
        "files": files
    })))
}

#[instrument(name = "Uploaded files", skip(value))]
async fn upload_backgrounded<R: FullRepo, S: Store>(
    value: Value<Backgrounded<R, S>>,
    repo: web::Data<R>,
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
        let upload_id = image.result.upload_id().expect("Upload ID exists");
        let identifier = image
            .result
            .identifier()
            .expect("Identifier exists")
            .to_bytes()?;

        queue::queue_ingest(&**repo, identifier, upload_id, None, true, false).await?;

        files.push(serde_json::json!({
            "upload_id": upload_id.to_string(),
        }));
    }

    for image in images {
        image.result.disarm();
    }

    Ok(HttpResponse::Accepted().json(&serde_json::json!({
        "msg": "ok",
        "uploads": files
    })))
}

#[derive(Debug, serde::Deserialize)]
struct ClaimQuery {
    upload_id: Serde<UploadId>,
}

/// Claim a backgrounded upload
#[instrument(name = "Waiting on upload", skip(repo))]
async fn claim_upload<R: FullRepo>(
    repo: web::Data<R>,
    query: web::Query<ClaimQuery>,
) -> Result<HttpResponse, Error> {
    let upload_id = Serde::into_inner(query.into_inner().upload_id);

    match actix_rt::time::timeout(Duration::from_secs(10), repo.wait(upload_id)).await {
        Ok(wait_res) => {
            let upload_result = wait_res?;
            repo.claim(upload_id).await?;

            match upload_result {
                UploadResult::Success { alias, token } => {
                    Ok(HttpResponse::Ok().json(&serde_json::json!({
                        "msg": "ok",
                        "files": [{
                            "file": alias.to_string(),
                            "delete_token": token.to_string(),
                        }]
                    })))
                }
                UploadResult::Failure { message } => Ok(HttpResponse::UnprocessableEntity().json(
                    &serde_json::json!({
                        "msg": message,
                    }),
                )),
            }
        }
        Err(_) => Ok(HttpResponse::NoContent().finish()),
    }
}

#[derive(Debug, serde::Deserialize)]
struct UrlQuery {
    url: String,

    #[serde(default)]
    backgrounded: bool,

    #[serde(default)]
    ephemeral: bool,
}

/// download an image from a URL
#[instrument(name = "Downloading file", skip(client, repo))]
async fn download<R: FullRepo + 'static, S: Store + 'static>(
    client: web::Data<Client>,
    repo: web::Data<R>,
    store: web::Data<S>,
    query: web::Query<UrlQuery>,
) -> Result<HttpResponse, Error> {
    if query.backgrounded {
        do_download_backgrounded(client, repo, store, &query.url, query.ephemeral).await
    } else {
        do_download_inline(client, repo, store, &query.url, query.ephemeral).await
    }
}

#[instrument(name = "Downloading file inline", skip(client, repo))]
async fn do_download_inline<R: FullRepo + 'static, S: Store + 'static>(
    client: web::Data<Client>,
    repo: web::Data<R>,
    store: web::Data<S>,
    url: &str,
    is_cached: bool,
) -> Result<HttpResponse, Error> {
    let res = client.get(url).send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()).into());
    }

    let stream = res
        .map_err(Error::from)
        .limit((CONFIG.media.max_file_size * MEGABYTES) as u64);

    let mut session = ingest::ingest(&**repo, &**store, stream, None, true, is_cached).await?;

    let alias = session.alias().expect("alias should exist").to_owned();
    let delete_token = session.delete_token().await?;

    let identifier = repo.identifier_from_alias::<S::Identifier>(&alias).await?;

    let details = repo.details(&identifier).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&alias);
        let new_details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
        repo.relate_details(&identifier, &new_details).await?;
        new_details
    };

    session.disarm();

    Ok(HttpResponse::Created().json(&serde_json::json!({
        "msg": "ok",
        "files": [{
            "file": alias.to_string(),
            "delete_token": delete_token.to_string(),
            "details": details,
        }]
    })))
}

#[instrument(name = "Downloading file in background", skip(client))]
async fn do_download_backgrounded<R: FullRepo + 'static, S: Store + 'static>(
    client: web::Data<Client>,
    repo: web::Data<R>,
    store: web::Data<S>,
    url: &str,
    is_cached: bool,
) -> Result<HttpResponse, Error> {
    let res = client.get(url).send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()).into());
    }

    let stream = res
        .map_err(Error::from)
        .limit((CONFIG.media.max_file_size * MEGABYTES) as u64);

    let backgrounded = Backgrounded::proxy((**repo).clone(), (**store).clone(), stream).await?;

    let upload_id = backgrounded.upload_id().expect("Upload ID exists");
    let identifier = backgrounded
        .identifier()
        .expect("Identifier exists")
        .to_bytes()?;

    queue::queue_ingest(&**repo, identifier, upload_id, None, true, is_cached).await?;

    backgrounded.disarm();

    Ok(HttpResponse::Accepted().json(&serde_json::json!({
        "msg": "ok",
        "uploads": [{
            "upload_id": upload_id.to_string(),
        }]
    })))
}

/// Delete aliases and files
#[instrument(name = "Deleting file", skip(repo))]
async fn delete<R: FullRepo>(
    repo: web::Data<R>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, Error> {
    let (token, alias) = path_entries.into_inner();

    let token = DeleteToken::from_existing(&token);
    let alias = Alias::from_existing(&alias);

    queue::cleanup_alias(&**repo, alias, token).await?;

    Ok(HttpResponse::NoContent().finish())
}

type ProcessQuery = Vec<(String, String)>;

fn prepare_process(
    query: web::Query<ProcessQuery>,
    ext: &str,
) -> Result<(ImageFormat, Alias, PathBuf, Vec<String>), Error> {
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
        return Err(UploadError::MissingAlias.into());
    }

    let alias = Alias::from_existing(&alias);

    let operations = operations
        .into_iter()
        .filter(|(k, _)| CONFIG.media.filters.contains(&k.to_lowercase()))
        .collect::<Vec<_>>();

    let format = ext
        .parse::<ImageFormat>()
        .map_err(|_| UploadError::UnsupportedFormat)?;

    let ext = format.to_string();

    let (thumbnail_path, thumbnail_args) = self::processor::build_chain(&operations, &ext)?;

    Ok((format, alias, thumbnail_path, thumbnail_args))
}

#[instrument(name = "Fetching derived details", skip(repo))]
async fn process_details<R: FullRepo, S: Store>(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let (_, alias, thumbnail_path, _) = prepare_process(query, ext.as_str())?;

    let hash = repo.hash(&alias).await?;
    let identifier = repo
        .variant_identifier::<S::Identifier>(hash, thumbnail_path.to_string_lossy().to_string())
        .await?
        .ok_or(UploadError::MissingAlias)?;

    let details = repo.details(&identifier).await?;

    let details = details.ok_or(UploadError::NoFiles)?;

    Ok(HttpResponse::Ok().json(&details))
}

/// Process files
#[instrument(name = "Serving processed image", skip(repo))]
async fn process<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let (format, alias, thumbnail_path, thumbnail_args) = prepare_process(query, ext.as_str())?;

    repo.check_cached(&alias).await?;

    let path_string = thumbnail_path.to_string_lossy().to_string();
    let hash = repo.hash(&alias).await?;
    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if let Some(identifier) = identifier_opt {
        let details_opt = repo.details(&identifier).await?;

        let details = if let Some(details) = details_opt {
            details
        } else {
            let hint = details_hint(&alias);
            let details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
            repo.relate_details(&identifier, &details).await?;
            details
        };

        return ranged_file_resp(&**store, identifier, range, details).await;
    }

    let (details, bytes) = generate::generate(
        &**repo,
        &**store,
        format,
        alias,
        thumbnail_path,
        thumbnail_args,
        hash,
    )
    .await?;

    let (builder, stream) = if let Some(web::Header(range_header)) = range {
        if let Some(range) = range::single_bytes_range(&range_header) {
            let len = bytes.len() as u64;

            if let Some(content_range) = range::to_content_range(range, len) {
                let mut builder = HttpResponse::PartialContent();
                builder.insert_header(content_range);
                let stream = range::chop_bytes(range, bytes, len)?;

                (builder, Either::left(Either::left(stream)))
            } else {
                (
                    HttpResponse::RangeNotSatisfiable(),
                    Either::left(Either::right(empty())),
                )
            }
        } else {
            return Err(UploadError::Range.into());
        }
    } else {
        (HttpResponse::Ok(), Either::right(once(ready(Ok(bytes)))))
    };

    Ok(srv_response(
        builder,
        stream,
        details.content_type(),
        7 * DAYS,
        details.system_time(),
    ))
}

/// Process files
#[instrument(name = "Spawning image process", skip(repo))]
async fn process_backgrounded<R: FullRepo, S: Store>(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let (target_format, source, process_path, process_args) = prepare_process(query, ext.as_str())?;

    let path_string = process_path.to_string_lossy().to_string();
    let hash = repo.hash(&source).await?;
    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if identifier_opt.is_some() {
        return Ok(HttpResponse::Accepted().finish());
    }

    queue_generate(&**repo, target_format, source, process_path, process_args).await?;

    Ok(HttpResponse::Accepted().finish())
}

/// Fetch file details
#[instrument(name = "Fetching details", skip(repo))]
async fn details<R: FullRepo, S: Store + 'static>(
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = alias.into_inner();

    let identifier = repo.identifier_from_alias::<S::Identifier>(&alias).await?;

    let details = repo.details(&identifier).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&alias);
        let new_details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
        repo.relate_details(&identifier, &new_details).await?;
        new_details
    };

    Ok(HttpResponse::Ok().json(&details))
}

/// Serve files
#[instrument(name = "Serving file", skip(repo))]
async fn serve<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = alias.into_inner();

    repo.check_cached(&alias).await?;

    let identifier = repo.identifier_from_alias::<S::Identifier>(&alias).await?;

    let details = repo.details(&identifier).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&alias);
        let details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
        repo.relate_details(&identifier, &details).await?;
        details
    };

    ranged_file_resp(&**store, identifier, range, details).await
}

async fn ranged_file_resp<S: Store + 'static>(
    store: &S,
    identifier: S::Identifier,
    range: Option<web::Header<Range>>,
    details: Details,
) -> Result<HttpResponse, Error> {
    let (builder, stream) = if let Some(web::Header(range_header)) = range {
        //Range header exists - return as ranged
        if let Some(range) = range::single_bytes_range(&range_header) {
            let len = store.len(&identifier).await?;

            if let Some(content_range) = range::to_content_range(range, len) {
                let mut builder = HttpResponse::PartialContent();
                builder.insert_header(content_range);
                (
                    builder,
                    Either::left(Either::left(
                        range::chop_store(range, store, &identifier, len)
                            .await?
                            .map_err(Error::from),
                    )),
                )
            } else {
                (
                    HttpResponse::RangeNotSatisfiable(),
                    Either::left(Either::right(empty())),
                )
            }
        } else {
            return Err(UploadError::Range.into());
        }
    } else {
        //No Range header in the request - return the entire document
        let stream = store
            .to_stream(&identifier, None, None)
            .await?
            .map_err(Error::from);
        (HttpResponse::Ok(), Either::right(stream))
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
    let stream = stream.timeout(Duration::from_secs(5)).map(|res| match res {
        Ok(Ok(item)) => Ok(item),
        Ok(Err(e)) => Err(actix_web::Error::from(e)),
        Err(e) => Err(Error::from(e).into()),
    });

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
struct AliasQuery {
    alias: Serde<Alias>,
}

#[instrument(name = "Purging file", skip(repo))]
async fn purge<R: FullRepo>(
    query: web::Query<AliasQuery>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let alias = query.into_inner().alias;
    let aliases = repo.aliases_from_alias(&alias).await?;

    let hash = repo.hash(&alias).await?;
    queue::cleanup_hash(&**repo, hash).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>()
    })))
}

#[instrument(name = "Fetching aliases", skip(repo))]
async fn aliases<R: FullRepo>(
    query: web::Query<AliasQuery>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let alias = query.into_inner().alias;
    let aliases = repo.aliases_from_alias(&alias).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>()
    })))
}

fn transform_error(error: actix_form_data::Error) -> actix_web::Error {
    let error: Error = error.into();
    let error: actix_web::Error = error.into();
    error
}

fn build_client() -> awc::Client {
    Client::builder()
        .wrap(Tracing)
        .add_default_header(("User-Agent", "pict-rs v0.3.0-main"))
        .finish()
}

fn build_reqwest_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("pict-rs v0.3.0-main")
        .build()
}

fn next_worker_id() -> String {
    static WORKER_ID: AtomicU64 = AtomicU64::new(0);

    let next_id = WORKER_ID.fetch_add(1, Ordering::Relaxed);

    format!("{}-{}", CONFIG.server.worker_id, next_id)
}

async fn launch<R: FullRepo + Clone + 'static, S: Store + Clone + 'static>(
    repo: R,
    store: S,
) -> color_eyre::Result<()> {
    repo.requeue_in_progress(CONFIG.server.worker_id.as_bytes().to_vec())
        .await?;
    // Create a new Multipart Form validator
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let repo2 = repo.clone();
    let store2 = store.clone();
    let form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.media.max_file_size * MEGABYTES)
        .transform_error(transform_error)
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let repo = repo2.clone();
                let store = store2.clone();

                let span = tracing::info_span!("file-upload", ?filename);

                let stream = stream.map_err(Error::from);

                Box::pin(
                    async move { ingest::ingest(&repo, &store, stream, None, true, false).await }
                        .instrument(span),
                )
            })),
        );

    // Create a new Multipart Form validator for internal imports
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let repo2 = repo.clone();
    let store2 = store.clone();
    let import_form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.media.max_file_size * MEGABYTES)
        .transform_error(transform_error)
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let repo = repo2.clone();
                let store = store2.clone();

                let span = tracing::info_span!("file-import", ?filename);

                let stream = stream.map_err(Error::from);

                Box::pin(
                    async move {
                        ingest::ingest(
                            &repo,
                            &store,
                            stream,
                            Some(Alias::from_existing(&filename)),
                            !CONFIG.media.skip_validate_imports,
                            false,
                        )
                        .await
                    }
                    .instrument(span),
                )
            })),
        );

    // Create a new Multipart Form validator for backgrounded uploads
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let repo2 = repo.clone();
    let store2 = store.clone();
    let backgrounded_form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.media.max_file_size * MEGABYTES)
        .transform_error(transform_error)
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let repo = repo2.clone();
                let store = store2.clone();

                let span = tracing::info_span!("file-proxy", ?filename);

                let stream = stream.map_err(Error::from);

                Box::pin(
                    async move { Backgrounded::proxy(repo, store, stream).await }.instrument(span),
                )
            })),
        );

    HttpServer::new(move || {
        let store = store.clone();
        let repo = repo.clone();

        tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
            actix_rt::spawn(queue::process_cleanup(
                repo.clone(),
                store.clone(),
                next_worker_id(),
            ))
        });
        tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
            actix_rt::spawn(queue::process_images(
                repo.clone(),
                store.clone(),
                next_worker_id(),
            ))
        });

        App::new()
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .app_data(web::Data::new(repo))
            .app_data(web::Data::new(store))
            .app_data(web::Data::new(build_client()))
            .service(
                web::scope("/image")
                    .service(
                        web::resource("")
                            .guard(guard::Post())
                            .wrap(form.clone())
                            .route(web::post().to(upload::<R, S>)),
                    )
                    .service(
                        web::scope("/backgrounded")
                            .service(
                                web::resource("")
                                    .guard(guard::Post())
                                    .wrap(backgrounded_form.clone())
                                    .route(web::post().to(upload_backgrounded::<R, S>)),
                            )
                            .service(
                                web::resource("/claim").route(web::get().to(claim_upload::<R>)),
                            ),
                    )
                    .service(web::resource("/download").route(web::get().to(download::<R, S>)))
                    .service(
                        web::resource("/delete/{delete_token}/{filename}")
                            .route(web::delete().to(delete::<R>))
                            .route(web::get().to(delete::<R>)),
                    )
                    .service(
                        web::resource("/original/{filename}").route(web::get().to(serve::<R, S>)),
                    )
                    .service(web::resource("/process.{ext}").route(web::get().to(process::<R, S>)))
                    .service(
                        web::resource("/process_backgrounded.{ext}")
                            .route(web::get().to(process_backgrounded::<R, S>)),
                    )
                    .service(
                        web::scope("/details")
                            .service(
                                web::resource("/original/{filename}")
                                    .route(web::get().to(details::<R, S>)),
                            )
                            .service(
                                web::resource("/process.{ext}")
                                    .route(web::get().to(process_details::<R, S>)),
                            ),
                    ),
            )
            .service(
                web::scope("/internal")
                    .wrap(Internal(
                        CONFIG.server.api_key.as_ref().map(|s| s.to_owned()),
                    ))
                    .service(
                        web::resource("/import")
                            .wrap(import_form.clone())
                            .route(web::post().to(upload::<R, S>)),
                    )
                    .service(web::resource("/purge").route(web::post().to(purge::<R>)))
                    .service(web::resource("/aliases").route(web::get().to(aliases::<R>))),
            )
    })
    .bind(CONFIG.server.address)?
    .run()
    .await?;

    crate::tmp_file::remove_tmp_dir().await?;

    Ok(())
}

async fn migrate_inner<S1>(repo: &Repo, from: S1, to: &config::Store) -> color_eyre::Result<()>
where
    S1: Store,
{
    match to {
        config::Store::Filesystem(config::Filesystem { path }) => {
            let to = FileStore::build(path.clone(), repo.clone()).await?;
            match repo {
                Repo::Sled(repo) => migrate_store(repo, from, to).await?,
            }
        }
        config::Store::ObjectStorage(config::ObjectStorage {
            bucket_name,
            region,
            access_key,
            secret_key,
            security_token,
            session_token,
        }) => {
            let to = ObjectStore::build(
                bucket_name,
                region.as_ref().clone(),
                Some(access_key.clone()),
                Some(secret_key.clone()),
                security_token.clone(),
                session_token.clone(),
                repo.clone(),
                build_reqwest_client()?,
            )
            .await?;

            match repo {
                Repo::Sled(repo) => migrate_store(repo, from, to).await?,
            }
        }
    }

    Ok(())
}

#[actix_rt::main]
async fn main() -> color_eyre::Result<()> {
    init_tracing(&CONFIG.tracing)?;

    let repo = Repo::open(CONFIG.repo.clone())?;
    repo.from_db(CONFIG.old_db.path.clone()).await?;

    match (*OPERATION).clone() {
        Operation::Run => (),
        Operation::MigrateStore { from, to } => {
            match from {
                config::Store::Filesystem(config::Filesystem { path }) => {
                    let from = FileStore::build(path.clone(), repo.clone()).await?;
                    migrate_inner(&repo, from, &to).await?;
                }
                config::Store::ObjectStorage(config::ObjectStorage {
                    bucket_name,
                    region,
                    access_key,
                    secret_key,
                    security_token,
                    session_token,
                }) => {
                    let from = ObjectStore::build(
                        &bucket_name,
                        Serde::into_inner(region),
                        Some(access_key),
                        Some(secret_key),
                        security_token,
                        session_token,
                        repo.clone(),
                        build_reqwest_client()?,
                    )
                    .await?;

                    migrate_inner(&repo, from, &to).await?;
                }
            }

            return Ok(());
        }
    }

    match CONFIG.store.clone() {
        config::Store::Filesystem(config::Filesystem { path }) => {
            let store = FileStore::build(path, repo.clone()).await?;
            match repo {
                Repo::Sled(sled_repo) => launch(sled_repo, store).await,
            }
        }
        config::Store::ObjectStorage(config::ObjectStorage {
            bucket_name,
            region,
            access_key,
            secret_key,
            security_token,
            session_token,
        }) => {
            let store = ObjectStore::build(
                &bucket_name,
                Serde::into_inner(region),
                Some(access_key),
                Some(secret_key),
                security_token,
                session_token,
                repo.clone(),
                build_reqwest_client()?,
            )
            .await?;

            match repo {
                Repo::Sled(sled_repo) => launch(sled_repo, store).await,
            }
        }
    }
}

const STORE_MIGRATION_PROGRESS: &str = "store-migration-progress";

async fn migrate_store<R, S1, S2>(repo: &R, from: S1, to: S2) -> Result<(), Error>
where
    S1: Store,
    S2: Store,
    R: IdentifierRepo + HashRepo + SettingsRepo,
{
    let stream = repo.hashes().await;
    let mut stream = Box::pin(stream);

    while let Some(hash) = stream.next().await {
        let hash = hash?;
        if let Some(identifier) = repo
            .motion_identifier(hash.as_ref().to_vec().into())
            .await?
        {
            let new_identifier = migrate_file(&from, &to, &identifier).await?;
            migrate_details(repo, identifier, &new_identifier).await?;
            repo.relate_motion_identifier(hash.as_ref().to_vec().into(), &new_identifier)
                .await?;
        }

        for (variant, identifier) in repo.variants(hash.as_ref().to_vec().into()).await? {
            let new_identifier = migrate_file(&from, &to, &identifier).await?;
            migrate_details(repo, identifier, &new_identifier).await?;
            repo.relate_variant_identifier(hash.as_ref().to_vec().into(), variant, &new_identifier)
                .await?;
        }

        let identifier = repo.identifier(hash.as_ref().to_vec().into()).await?;
        let new_identifier = migrate_file(&from, &to, &identifier).await?;
        migrate_details(repo, identifier, &new_identifier).await?;
        repo.relate_identifier(hash.as_ref().to_vec().into(), &new_identifier)
            .await?;

        repo.set(STORE_MIGRATION_PROGRESS, hash.as_ref().to_vec().into())
            .await?;
    }

    // clean up the migration key to avoid interfering with future migrations
    repo.remove(STORE_MIGRATION_PROGRESS).await?;

    Ok(())
}

async fn migrate_file<S1, S2>(
    from: &S1,
    to: &S2,
    identifier: &S1::Identifier,
) -> Result<S2::Identifier, Error>
where
    S1: Store,
    S2: Store,
{
    let stream = from.to_stream(identifier, None, None).await?;
    futures_util::pin_mut!(stream);
    let mut reader = tokio_util::io::StreamReader::new(stream);

    let new_identifier = to.save_async_read(&mut reader).await?;

    Ok(new_identifier)
}

async fn migrate_details<R, I1, I2>(repo: &R, from: I1, to: &I2) -> Result<(), Error>
where
    R: IdentifierRepo,
    I1: Identifier,
    I2: Identifier,
{
    if let Some(details) = repo.details(&from).await? {
        repo.relate_details(to, &details).await?;
        repo.cleanup(&from).await?;
    }

    Ok(())
}
