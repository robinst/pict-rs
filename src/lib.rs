mod backgrounded;
mod bytes_stream;
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

use actix_form_data::{Field, Form, FormData, Multipart, Value};
use actix_web::{
    guard,
    http::header::{CacheControl, CacheDirective, LastModified, Range, ACCEPT_RANGES},
    web, App, HttpRequest, HttpResponse, HttpResponseBuilder, HttpServer,
};
use awc::{Client, Connector};
use futures_util::{
    stream::{empty, once},
    Stream, StreamExt, TryStreamExt,
};
use once_cell::sync::{Lazy, OnceCell};
use rusty_s3::UrlStyle;
use std::{
    future::ready,
    path::Path,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::Semaphore;
use tracing_actix_web::TracingLogger;
use tracing_awc::Tracing;
use tracing_futures::Instrument;

use self::{
    backgrounded::Backgrounded,
    config::{Configuration, ImageFormat, Operation},
    details::Details,
    either::Either,
    error::{Error, UploadError},
    ingest::Session,
    init_tracing::init_tracing,
    magick::{details_hint, ValidInputType},
    middleware::{Deadline, Internal},
    queue::queue_generate,
    repo::{
        Alias, DeleteToken, FullRepo, HashRepo, IdentifierRepo, QueueRepo, Repo, SettingsRepo,
        UploadId, UploadResult,
    },
    serde_str::Serde,
    store::{
        file_store::FileStore,
        object_store::{ObjectStore, ObjectStoreConfig},
        Identifier, Store,
    },
    stream::{StreamLimit, StreamTimeout},
};

pub use self::config::ConfigSource;

const MEGABYTES: usize = 1024 * 1024;
const MINUTES: u32 = 60;
const HOURS: u32 = 60 * MINUTES;
const DAYS: u32 = 24 * HOURS;

const NOT_FOUND_KEY: &str = "404-alias";

static DO_CONFIG: OnceCell<(Configuration, Operation)> = OnceCell::new();
static CONFIG: Lazy<Configuration> = Lazy::new(|| {
    DO_CONFIG
        .get_or_try_init(config::configure)
        .expect("Failed to configure")
        .0
        .clone()
});
static OPERATION: Lazy<Operation> = Lazy::new(|| {
    DO_CONFIG
        .get_or_try_init(config::configure)
        .expect("Failed to configure")
        .1
        .clone()
});
static PROCESS_SEMAPHORE: Lazy<Semaphore> = Lazy::new(|| {
    tracing::trace_span!(parent: None, "Initialize semaphore")
        .in_scope(|| Semaphore::new(num_cpus::get().saturating_sub(1).max(1)))
});

async fn ensure_details<R: FullRepo, S: Store + 'static>(
    repo: &R,
    store: &S,
    alias: &Alias,
) -> Result<Details, Error> {
    let Some(identifier) = repo.identifier_from_alias::<S::Identifier>(alias).await? else {
        return Err(UploadError::MissingAlias.into());
    };

    let details = repo.details(&identifier).await?;

    if let Some(details) = details {
        tracing::debug!("details exist");
        Ok(details)
    } else {
        tracing::debug!("generating new details from {:?}", identifier);
        let hint = details_hint(alias);
        let new_details = Details::from_store(store.clone(), identifier.clone(), hint).await?;
        tracing::debug!("storing details for {:?}", identifier);
        repo.relate_details(&identifier, &new_details).await?;
        tracing::debug!("stored");
        Ok(new_details)
    }
}

struct Upload<R: FullRepo + 'static, S: Store + 'static>(Value<Session<R, S>>);

impl<R: FullRepo, S: Store + 'static> FormData for Upload<R, S> {
    type Item = Session<R, S>;
    type Error = Error;

    fn form(req: &HttpRequest) -> Form<Self::Item, Self::Error> {
        // Create a new Multipart Form validator
        //
        // This form is expecting a single array field, 'images' with at most 10 files in it
        let repo = req
            .app_data::<web::Data<R>>()
            .expect("No repo in request")
            .clone();
        let store = req
            .app_data::<web::Data<S>>()
            .expect("No store in request")
            .clone();

        Form::new()
            .max_files(10)
            .max_file_size(CONFIG.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let repo = repo.clone();
                    let store = store.clone();

                    let span = tracing::info_span!("file-upload", ?filename);

                    let stream = stream.map_err(Error::from);

                    Box::pin(
                        async move { ingest::ingest(&**repo, &**store, stream, None, true).await }
                            .instrument(span),
                    )
                })),
            )
    }

    fn extract(value: Value<Session<R, S>>) -> Result<Self, Self::Error> {
        Ok(Upload(value))
    }
}

struct Import<R: FullRepo + 'static, S: Store + 'static>(Value<Session<R, S>>);

impl<R: FullRepo, S: Store + 'static> FormData for Import<R, S> {
    type Item = Session<R, S>;
    type Error = Error;

    fn form(req: &actix_web::HttpRequest) -> Form<Self::Item, Self::Error> {
        let repo = req
            .app_data::<web::Data<R>>()
            .expect("No repo in request")
            .clone();
        let store = req
            .app_data::<web::Data<S>>()
            .expect("No store in request")
            .clone();

        // Create a new Multipart Form validator for internal imports
        //
        // This form is expecting a single array field, 'images' with at most 10 files in it
        Form::new()
            .max_files(10)
            .max_file_size(CONFIG.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let repo = repo.clone();
                    let store = store.clone();

                    let span = tracing::info_span!("file-import", ?filename);

                    let stream = stream.map_err(Error::from);

                    Box::pin(
                        async move {
                            ingest::ingest(
                                &**repo,
                                &**store,
                                stream,
                                Some(Alias::from_existing(&filename)),
                                !CONFIG.media.skip_validate_imports,
                            )
                            .await
                        }
                        .instrument(span),
                    )
                })),
            )
    }

    fn extract(value: Value<Self::Item>) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Ok(Import(value))
    }
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Uploaded files", skip(value, repo, store))]
async fn upload<R: FullRepo, S: Store + 'static>(
    Multipart(Upload(value)): Multipart<Upload<R, S>>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    handle_upload(value, repo, store).await
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Imported files", skip(value, repo, store))]
async fn import<R: FullRepo, S: Store + 'static>(
    Multipart(Import(value)): Multipart<Import<R, S>>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    handle_upload(value, repo, store).await
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Uploaded files", skip(value, repo, store))]
async fn handle_upload<R: FullRepo, S: Store + 'static>(
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
            tracing::debug!("Uploaded {} as {:?}", image.filename, alias);
            let delete_token = image.result.delete_token().await?;

            let details = ensure_details(&repo, &store, alias).await?;

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

struct BackgroundedUpload<R: FullRepo + 'static, S: Store + 'static>(Value<Backgrounded<R, S>>);

impl<R: FullRepo, S: Store + 'static> FormData for BackgroundedUpload<R, S> {
    type Item = Backgrounded<R, S>;
    type Error = Error;

    fn form(req: &actix_web::HttpRequest) -> Form<Self::Item, Self::Error> {
        // Create a new Multipart Form validator for backgrounded uploads
        //
        // This form is expecting a single array field, 'images' with at most 10 files in it
        let repo = req
            .app_data::<web::Data<R>>()
            .expect("No repo in request")
            .clone();
        let store = req
            .app_data::<web::Data<S>>()
            .expect("No store in request")
            .clone();

        Form::new()
            .max_files(10)
            .max_file_size(CONFIG.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let repo = (**repo).clone();
                    let store = (**store).clone();

                    let span = tracing::info_span!("file-proxy", ?filename);

                    let stream = stream.map_err(Error::from);

                    Box::pin(
                        async move { Backgrounded::proxy(repo, store, stream).await }
                            .instrument(span),
                    )
                })),
            )
    }

    fn extract(value: Value<Self::Item>) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Ok(BackgroundedUpload(value))
    }
}

#[tracing::instrument(name = "Uploaded files", skip(value, repo))]
async fn upload_backgrounded<R: FullRepo, S: Store>(
    Multipart(BackgroundedUpload(value)): Multipart<BackgroundedUpload<R, S>>,
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

        queue::queue_ingest(&repo, identifier, upload_id, None, true).await?;

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
#[tracing::instrument(name = "Waiting on upload", skip_all)]
async fn claim_upload<R: FullRepo, S: Store + 'static>(
    repo: web::Data<R>,
    store: web::Data<S>,
    query: web::Query<ClaimQuery>,
) -> Result<HttpResponse, Error> {
    let upload_id = Serde::into_inner(query.into_inner().upload_id);

    match actix_rt::time::timeout(Duration::from_secs(10), repo.wait(upload_id)).await {
        Ok(wait_res) => {
            let upload_result = wait_res?;
            repo.claim(upload_id).await?;

            match upload_result {
                UploadResult::Success { alias, token } => {
                    let details = ensure_details(&repo, &store, &alias).await?;

                    Ok(HttpResponse::Ok().json(&serde_json::json!({
                        "msg": "ok",
                        "files": [{
                            "file": alias.to_string(),
                            "delete_token": token.to_string(),
                            "details": details,
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
}

/// download an image from a URL
#[tracing::instrument(name = "Downloading file", skip(client, repo, store))]
async fn download<R: FullRepo + 'static, S: Store + 'static>(
    client: web::Data<Client>,
    repo: web::Data<R>,
    store: web::Data<S>,
    query: web::Query<UrlQuery>,
) -> Result<HttpResponse, Error> {
    let res = client.get(&query.url).send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()).into());
    }

    let stream = res
        .map_err(Error::from)
        .limit((CONFIG.media.max_file_size * MEGABYTES) as u64);

    if query.backgrounded {
        do_download_backgrounded(stream, repo, store).await
    } else {
        do_download_inline(stream, repo, store).await
    }
}

#[tracing::instrument(name = "Downloading file inline", skip(stream, repo, store))]
async fn do_download_inline<R: FullRepo + 'static, S: Store + 'static>(
    stream: impl Stream<Item = Result<web::Bytes, Error>> + Unpin + 'static,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let mut session = ingest::ingest(&repo, &store, stream, None, true).await?;

    let alias = session.alias().expect("alias should exist").to_owned();
    let delete_token = session.delete_token().await?;

    let details = ensure_details(&repo, &store, &alias).await?;

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

#[tracing::instrument(name = "Downloading file in background", skip(stream, repo, store))]
async fn do_download_backgrounded<R: FullRepo + 'static, S: Store + 'static>(
    stream: impl Stream<Item = Result<web::Bytes, Error>> + Unpin + 'static,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let backgrounded = Backgrounded::proxy((**repo).clone(), (**store).clone(), stream).await?;

    let upload_id = backgrounded.upload_id().expect("Upload ID exists");
    let identifier = backgrounded
        .identifier()
        .expect("Identifier exists")
        .to_bytes()?;

    queue::queue_ingest(&repo, identifier, upload_id, None, true).await?;

    backgrounded.disarm();

    Ok(HttpResponse::Accepted().json(&serde_json::json!({
        "msg": "ok",
        "uploads": [{
            "upload_id": upload_id.to_string(),
        }]
    })))
}

/// Delete aliases and files
#[tracing::instrument(name = "Deleting file", skip(repo))]
async fn delete<R: FullRepo>(
    repo: web::Data<R>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, Error> {
    let (token, alias) = path_entries.into_inner();

    let token = DeleteToken::from_existing(&token);
    let alias = Alias::from_existing(&alias);

    queue::cleanup_alias(&repo, alias, token).await?;

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

#[tracing::instrument(name = "Fetching derived details", skip(repo))]
async fn process_details<R: FullRepo, S: Store>(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let (_, alias, thumbnail_path, _) = prepare_process(query, ext.as_str())?;

    let Some(hash) = repo.hash(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().json(&serde_json::json!({
            "msg": "No images associated with provided alias",
        })));
    };

    let identifier = repo
        .variant_identifier::<S::Identifier>(hash, thumbnail_path.to_string_lossy().to_string())
        .await?
        .ok_or(UploadError::MissingAlias)?;

    let details = repo.details(&identifier).await?;

    let details = details.ok_or(UploadError::NoFiles)?;

    Ok(HttpResponse::Ok().json(&details))
}

async fn not_found_hash<R: FullRepo>(repo: &R) -> Result<Option<(Alias, R::Bytes)>, Error> {
    let Some(not_found) = repo.get(NOT_FOUND_KEY).await? else {
        return Ok(None);
    };

    let Some(alias) = Alias::from_slice(not_found.as_ref()) else {
        tracing::warn!("Couldn't parse not-found alias");
        return Ok(None);
    };

    let Some(hash) = repo.hash(&alias).await? else {
        tracing::warn!("No hash found for not-found alias");
        return Ok(None);
    };

    Ok(Some((alias, hash)))
}

/// Process files
#[tracing::instrument(name = "Serving processed image", skip(repo, store))]
async fn process<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let (format, alias, thumbnail_path, thumbnail_args) = prepare_process(query, ext.as_str())?;

    let path_string = thumbnail_path.to_string_lossy().to_string();

    let (hash, alias, not_found) = if let Some(hash) = repo.hash(&alias).await? {
        (hash, alias, false)
    } else {
        let Some((alias, hash)) = not_found_hash(&repo).await? else {
            return Ok(HttpResponse::NotFound().finish());
        };

        (hash, alias, true)
    };

    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if let Some(identifier) = identifier_opt {
        let details = repo.details(&identifier).await?;

        let details = if let Some(details) = details {
            tracing::debug!("details exist");
            details
        } else {
            tracing::debug!("generating new details from {:?}", identifier);
            let new_details = Details::from_store(
                (**store).clone(),
                identifier.clone(),
                Some(ValidInputType::from_format(format)),
            )
            .await?;
            tracing::debug!("storing details for {:?}", identifier);
            repo.relate_details(&identifier, &new_details).await?;
            tracing::debug!("stored");
            new_details
        };

        return ranged_file_resp(&store, identifier, range, details, not_found).await;
    }

    let original_details = ensure_details(&repo, &store, &alias).await?;

    let (details, bytes) = generate::generate(
        &repo,
        &store,
        format,
        alias,
        thumbnail_path,
        thumbnail_args,
        original_details.to_input_format(),
        None,
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
    } else if not_found {
        (
            HttpResponse::NotFound(),
            Either::right(once(ready(Ok(bytes)))),
        )
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

#[tracing::instrument(name = "Serving processed image headers", skip(repo, store))]
async fn process_head<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let (format, alias, thumbnail_path, _) = prepare_process(query, ext.as_str())?;

    let path_string = thumbnail_path.to_string_lossy().to_string();
    let Some(hash) = repo.hash(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().finish());
    };

    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if let Some(identifier) = identifier_opt {
        let details = repo.details(&identifier).await?;

        let details = if let Some(details) = details {
            tracing::debug!("details exist");
            details
        } else {
            tracing::debug!("generating new details from {:?}", identifier);
            let new_details = Details::from_store(
                (**store).clone(),
                identifier.clone(),
                Some(ValidInputType::from_format(format)),
            )
            .await?;
            tracing::debug!("storing details for {:?}", identifier);
            repo.relate_details(&identifier, &new_details).await?;
            tracing::debug!("stored");
            new_details
        };

        return ranged_file_head_resp(&store, identifier, range, details).await;
    }

    Ok(HttpResponse::NotFound().finish())
}

/// Process files
#[tracing::instrument(name = "Spawning image process", skip(repo))]
async fn process_backgrounded<R: FullRepo, S: Store>(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let (target_format, source, process_path, process_args) = prepare_process(query, ext.as_str())?;

    let path_string = process_path.to_string_lossy().to_string();
    let Some(hash) = repo.hash(&source).await? else {
        // Invalid alias
        return Ok(HttpResponse::BadRequest().finish());
    };

    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if identifier_opt.is_some() {
        return Ok(HttpResponse::Accepted().finish());
    }

    queue_generate(&repo, target_format, source, process_path, process_args).await?;

    Ok(HttpResponse::Accepted().finish())
}

/// Fetch file details
#[tracing::instrument(name = "Fetching details", skip(repo, store))]
async fn details<R: FullRepo, S: Store + 'static>(
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = alias.into_inner();

    let details = ensure_details(&repo, &store, &alias).await?;

    Ok(HttpResponse::Ok().json(&details))
}

/// Serve files
#[tracing::instrument(name = "Serving file", skip(repo, store))]
async fn serve<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = alias.into_inner();

    let (hash, alias, not_found) = if let Some(hash) = repo.hash(&alias).await? {
        (hash, Serde::into_inner(alias), false)
    } else {
        let Some((alias, hash)) = not_found_hash(&repo).await? else {
            return Ok(HttpResponse::NotFound().finish());
        };

        (hash, alias, true)
    };

    let Some(identifier) = repo.identifier(hash.clone()).await? else {
        tracing::warn!(
            "Original File identifier for hash {} is missing, queue cleanup task",
            hex::encode(&hash)
        );
        crate::queue::cleanup_hash(&repo, hash).await?;
        return Ok(HttpResponse::NotFound().finish());
    };

    let details = ensure_details(&repo, &store, &alias).await?;

    ranged_file_resp(&store, identifier, range, details, not_found).await
}

#[tracing::instrument(name = "Serving file headers", skip(repo, store))]
async fn serve_head<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = alias.into_inner();

    let Some(identifier) = repo.identifier_from_alias::<S::Identifier>(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().finish());
    };

    let details = ensure_details(&repo, &store, &alias).await?;

    ranged_file_head_resp(&store, identifier, range, details).await
}

async fn ranged_file_head_resp<S: Store + 'static>(
    store: &S,
    identifier: S::Identifier,
    range: Option<web::Header<Range>>,
    details: Details,
) -> Result<HttpResponse, Error> {
    let builder = if let Some(web::Header(range_header)) = range {
        //Range header exists - return as ranged
        if let Some(range) = range::single_bytes_range(&range_header) {
            let len = store.len(&identifier).await?;

            if let Some(content_range) = range::to_content_range(range, len) {
                let mut builder = HttpResponse::PartialContent();
                builder.insert_header(content_range);
                builder
            } else {
                HttpResponse::RangeNotSatisfiable()
            }
        } else {
            return Err(UploadError::Range.into());
        }
    } else {
        // no range header
        HttpResponse::Ok()
    };

    Ok(srv_head(
        builder,
        details.content_type(),
        7 * DAYS,
        details.system_time(),
    )
    .finish())
}

async fn ranged_file_resp<S: Store + 'static>(
    store: &S,
    identifier: S::Identifier,
    range: Option<web::Header<Range>>,
    details: Details,
    not_found: bool,
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

        if not_found {
            (HttpResponse::NotFound(), Either::right(stream))
        } else {
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
    builder: HttpResponseBuilder,
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

    srv_head(builder, ext, expires, modified).streaming(stream)
}

// A helper method to produce responses with proper cache headers
fn srv_head(
    mut builder: HttpResponseBuilder,
    ext: mime::Mime,
    expires: u32,
    modified: SystemTime,
) -> HttpResponseBuilder {
    builder
        .insert_header(LastModified(modified.into()))
        .insert_header(CacheControl(vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(expires),
            CacheDirective::Extension("immutable".to_owned(), None),
        ]))
        .insert_header((ACCEPT_RANGES, "bytes"))
        .content_type(ext.to_string());

    builder
}

#[tracing::instrument(name = "Spawning variant cleanup", skip(repo))]
async fn clean_variants<R: FullRepo>(repo: web::Data<R>) -> Result<HttpResponse, Error> {
    queue::cleanup_all_variants(&repo).await?;
    Ok(HttpResponse::NoContent().finish())
}

#[derive(Debug, serde::Deserialize)]
struct AliasQuery {
    alias: Serde<Alias>,
}

#[tracing::instrument(name = "Setting 404 Image", skip(repo))]
async fn set_not_found<R: FullRepo>(
    json: web::Json<AliasQuery>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let alias = json.into_inner().alias;

    if repo.hash(&alias).await?.is_none() {
        return Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "msg": "No hash associated with provided alias"
        })));
    }

    repo.set(NOT_FOUND_KEY, alias.to_bytes().into()).await?;

    Ok(HttpResponse::Created().json(serde_json::json!({
        "msg": "ok",
    })))
}

#[tracing::instrument(name = "Purging file", skip(repo))]
async fn purge<R: FullRepo>(
    query: web::Query<AliasQuery>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let alias = query.into_inner().alias;
    let aliases = repo.aliases_from_alias(&alias).await?;

    let Some(hash) = repo.hash(&alias).await? else {
        return Ok(HttpResponse::BadRequest().json(&serde_json::json!({
            "msg": "No images associated with provided alias",
        })));
    };
    queue::cleanup_hash(&repo, hash).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>()
    })))
}

#[tracing::instrument(name = "Fetching aliases", skip(repo))]
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

#[tracing::instrument(name = "Fetching identifier", skip(repo))]
async fn identifier<R: FullRepo, S: Store>(
    query: web::Query<AliasQuery>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let alias = query.into_inner().alias;
    let Some(identifier) = repo.identifier_from_alias::<S::Identifier>(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().json(serde_json::json!({
            "msg": "No identifiers associated with provided alias"
        })));
    };

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "identifier": identifier.string_repr(),
    })))
}

async fn healthz<R: FullRepo, S: Store>(
    repo: web::Data<R>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    repo.health_check().await?;
    store.health_check().await?;
    Ok(HttpResponse::Ok().finish())
}

fn transform_error(error: actix_form_data::Error) -> actix_web::Error {
    let error: Error = error.into();
    let error: actix_web::Error = error.into();
    error
}

fn build_client() -> awc::Client {
    let connector = CONFIG
        .server
        .client_pool_size
        .map(|size| Connector::new().limit(size))
        .unwrap_or_else(Connector::new);

    Client::builder()
        .connector(connector)
        .wrap(Tracing)
        .add_default_header(("User-Agent", "pict-rs v0.4.0-main"))
        .timeout(Duration::from_secs(30))
        .finish()
}

fn next_worker_id() -> String {
    static WORKER_ID: AtomicU64 = AtomicU64::new(0);

    let next_id = WORKER_ID.fetch_add(1, Ordering::Relaxed);

    format!("{}-{}", CONFIG.server.worker_id, next_id)
}

fn configure_endpoints<R: FullRepo + 'static, S: Store + 'static>(
    config: &mut web::ServiceConfig,
    repo: R,
    store: S,
    client: Client,
) {
    config
        .app_data(web::Data::new(repo))
        .app_data(web::Data::new(store))
        .app_data(web::Data::new(client))
        .route("/healthz", web::get().to(healthz::<R, S>))
        .service(
            web::scope("/image")
                .service(
                    web::resource("")
                        .guard(guard::Post())
                        .route(web::post().to(upload::<R, S>)),
                )
                .service(
                    web::scope("/backgrounded")
                        .service(
                            web::resource("")
                                .guard(guard::Post())
                                .route(web::post().to(upload_backgrounded::<R, S>)),
                        )
                        .service(
                            web::resource("/claim").route(web::get().to(claim_upload::<R, S>)),
                        ),
                )
                .service(web::resource("/download").route(web::get().to(download::<R, S>)))
                .service(
                    web::resource("/delete/{delete_token}/{filename}")
                        .route(web::delete().to(delete::<R>))
                        .route(web::get().to(delete::<R>)),
                )
                .service(
                    web::resource("/original/{filename}")
                        .route(web::get().to(serve::<R, S>))
                        .route(web::head().to(serve_head::<R, S>)),
                )
                .service(
                    web::resource("/process.{ext}")
                        .route(web::get().to(process::<R, S>))
                        .route(web::head().to(process_head::<R, S>)),
                )
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
                .service(web::resource("/import").route(web::post().to(import::<R, S>)))
                .service(web::resource("/variants").route(web::delete().to(clean_variants::<R>)))
                .service(web::resource("/purge").route(web::post().to(purge::<R>)))
                .service(web::resource("/aliases").route(web::get().to(aliases::<R>)))
                .service(web::resource("/identifier").route(web::get().to(identifier::<R, S>)))
                .service(web::resource("/set_not_found").route(web::post().to(set_not_found::<R>))),
        );
}

fn spawn_workers<R, S>(repo: R, store: S)
where
    R: FullRepo + 'static,
    S: Store + 'static,
{
    tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
        actix_rt::spawn(queue::process_cleanup(
            repo.clone(),
            store.clone(),
            next_worker_id(),
        ))
    });
    tracing::trace_span!(parent: None, "Spawn task")
        .in_scope(|| actix_rt::spawn(queue::process_images(repo, store, next_worker_id())));
}

async fn launch_file_store<R: FullRepo + 'static>(
    repo: R,
    store: FileStore,
) -> std::io::Result<()> {
    HttpServer::new(move || {
        let client = build_client();

        let store = store.clone();
        let repo = repo.clone();

        spawn_workers(repo.clone(), store.clone());

        App::new()
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .configure(move |sc| configure_endpoints(sc, repo, store, client))
    })
    .bind(CONFIG.server.address)?
    .run()
    .await
}

async fn launch_object_store<R: FullRepo + 'static>(
    repo: R,
    store_config: ObjectStoreConfig,
) -> std::io::Result<()> {
    HttpServer::new(move || {
        let client = build_client();

        let store = store_config.clone().build(client.clone());
        let repo = repo.clone();

        spawn_workers(repo.clone(), store.clone());

        App::new()
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .configure(move |sc| configure_endpoints(sc, repo, store, client))
    })
    .bind(CONFIG.server.address)?
    .run()
    .await
}

async fn migrate_inner<S1>(
    repo: &Repo,
    client: Client,
    from: S1,
    to: config::Store,
    skip_missing_files: bool,
) -> color_eyre::Result<()>
where
    S1: Store,
{
    match to {
        config::Store::Filesystem(config::Filesystem { path }) => {
            let to = FileStore::build(path.clone(), repo.clone()).await?;

            match repo {
                Repo::Sled(repo) => migrate_store(repo, from, to, skip_missing_files).await?,
            }
        }
        config::Store::ObjectStorage(config::ObjectStorage {
            endpoint,
            bucket_name,
            use_path_style,
            region,
            access_key,
            secret_key,
            session_token,
        }) => {
            let to = ObjectStore::build(
                endpoint.clone(),
                bucket_name,
                if use_path_style {
                    UrlStyle::Path
                } else {
                    UrlStyle::VirtualHost
                },
                region,
                access_key,
                secret_key,
                session_token,
                repo.clone(),
            )
            .await?
            .build(client);

            match repo {
                Repo::Sled(repo) => migrate_store(repo, from, to, skip_missing_files).await?,
            }
        }
    }

    Ok(())
}

impl<P: AsRef<Path>, T: serde::Serialize> ConfigSource<P, T> {
    /// Initialize the pict-rs configuration
    ///
    /// This takes an optional config_file path which is a valid pict-rs configuration file, and an
    /// optional save_to path, which the generated configuration will be saved into. Since many
    /// parameters have defaults, it can be useful to dump a valid configuration with default values to
    /// see what is available for tweaking.
    ///
    /// This function must be called before `run` or `install_tracing`
    ///
    /// When running pict-rs as a library, configuration is limited to environment variables and
    /// configuration files. Commandline options are not available.
    ///
    /// ```rust
    /// fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     pict_rs::ConfigSource::memory(serde_json::json!({
    ///         "server": {
    ///             "address": "127.0.0.1:8080"
    ///         },
    ///         "old_db": {
    ///             "path": "./old"
    ///         },
    ///         "repo": {
    ///             "type": "sled",
    ///             "path": "./sled-repo"
    ///         },
    ///         "store": {
    ///             "type": "filesystem",
    ///             "path": "./files"
    ///         }
    ///     })).init::<&str>(None)?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn init<Q: AsRef<Path>>(self, save_to: Option<Q>) -> color_eyre::Result<()> {
        let (config, operation) = config::configure_without_clap(self, save_to)?;

        DO_CONFIG
            .set((config, operation))
            .unwrap_or_else(|_| panic!("CONFIG cannot be initialized more than once"));

        Ok(())
    }
}

/// Install the default pict-rs tracer
///
/// This is probably not useful for 3rd party applications that install their own tracing
/// subscribers.
pub fn install_tracing() -> color_eyre::Result<()> {
    init_tracing(&CONFIG.tracing)
}

/// Run the pict-rs application
///
/// This must be called after `init_config`, or else the default configuration builder will run and
/// fail.
pub async fn run() -> color_eyre::Result<()> {
    let repo = Repo::open(CONFIG.repo.clone())?;
    repo.migrate_from_db(CONFIG.old_db.path.clone()).await?;

    match (*OPERATION).clone() {
        Operation::Run => (),
        Operation::MigrateStore {
            skip_missing_files,
            from,
            to,
        } => {
            let client = build_client();

            match from {
                config::Store::Filesystem(config::Filesystem { path }) => {
                    let from = FileStore::build(path.clone(), repo.clone()).await?;
                    migrate_inner(&repo, client, from, to, skip_missing_files).await?;
                }
                config::Store::ObjectStorage(config::ObjectStorage {
                    endpoint,
                    bucket_name,
                    use_path_style,
                    region,
                    access_key,
                    secret_key,
                    session_token,
                }) => {
                    let from = ObjectStore::build(
                        endpoint,
                        bucket_name,
                        if use_path_style {
                            UrlStyle::Path
                        } else {
                            UrlStyle::VirtualHost
                        },
                        region,
                        access_key,
                        secret_key,
                        session_token,
                        repo.clone(),
                    )
                    .await?
                    .build(client.clone());

                    migrate_inner(&repo, client, from, to, skip_missing_files).await?;
                }
            }

            return Ok(());
        }
    }

    match CONFIG.store.clone() {
        config::Store::Filesystem(config::Filesystem { path }) => {
            repo.migrate_identifiers().await?;

            let store = FileStore::build(path, repo.clone()).await?;
            match repo {
                Repo::Sled(sled_repo) => {
                    sled_repo
                        .requeue_in_progress(CONFIG.server.worker_id.as_bytes().to_vec())
                        .await?;

                    launch_file_store(sled_repo, store).await?;
                }
            }
        }
        config::Store::ObjectStorage(config::ObjectStorage {
            endpoint,
            bucket_name,
            use_path_style,
            region,
            access_key,
            secret_key,
            session_token,
        }) => {
            let store = ObjectStore::build(
                endpoint,
                bucket_name,
                if use_path_style {
                    UrlStyle::Path
                } else {
                    UrlStyle::VirtualHost
                },
                region,
                access_key,
                secret_key,
                session_token,
                repo.clone(),
            )
            .await?;

            match repo {
                Repo::Sled(sled_repo) => {
                    sled_repo
                        .requeue_in_progress(CONFIG.server.worker_id.as_bytes().to_vec())
                        .await?;

                    launch_object_store(sled_repo, store).await?;
                }
            }
        }
    }

    self::tmp_file::remove_tmp_dir().await?;

    Ok(())
}

const STORE_MIGRATION_PROGRESS: &str = "store-migration-progress";
const STORE_MIGRATION_MOTION: &str = "store-migration-motion";
const STORE_MIGRATION_VARIANT: &str = "store-migration-variant";

async fn migrate_store<R, S1, S2>(
    repo: &R,
    from: S1,
    to: S2,
    skip_missing_files: bool,
) -> Result<(), Error>
where
    S1: Store + Clone,
    S2: Store + Clone,
    R: IdentifierRepo + HashRepo + SettingsRepo + QueueRepo,
{
    tracing::warn!("Running checks");

    if let Err(e) = from.health_check().await {
        tracing::warn!("Old store is not configured correctly");
        return Err(e.into());
    }
    if let Err(e) = to.health_check().await {
        tracing::warn!("New store is not configured correctly");
        return Err(e.into());
    }

    tracing::warn!("Checks complete, migrating store");

    let mut failure_count = 0;

    while let Err(e) = do_migrate_store(repo, from.clone(), to.clone(), skip_missing_files).await {
        tracing::error!("Migration failed with {}", format!("{e:?}"));

        failure_count += 1;

        if failure_count >= 50 {
            tracing::error!("Exceeded 50 errors");
            return Err(e);
        } else {
            tracing::warn!("Retrying migration +{failure_count}");
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    Ok(())
}
async fn do_migrate_store<R, S1, S2>(
    repo: &R,
    from: S1,
    to: S2,
    skip_missing_files: bool,
) -> Result<(), Error>
where
    S1: Store,
    S2: Store,
    R: IdentifierRepo + HashRepo + SettingsRepo + QueueRepo,
{
    let mut repo_size = repo.size().await?;

    let mut progress_opt = repo.get(STORE_MIGRATION_PROGRESS).await?;

    if progress_opt.is_some() {
        tracing::warn!("Continuing previous migration of {repo_size} total hashes");
    } else {
        tracing::warn!("{repo_size} hashes will be migrated");
    }

    if repo_size == 0 {
        return Ok(());
    }

    let mut pct = repo_size / 100;

    // Hashes are read in a consistent order
    let stream = repo.hashes().await;
    let mut stream = Box::pin(stream);

    let now = Instant::now();
    let mut index = 0;
    while let Some(hash) = stream.next().await {
        index += 1;

        let hash = hash?;

        if let Some(progress) = &progress_opt {
            // we've reached the most recently migrated hash.
            if progress.as_ref() == hash.as_ref() {
                progress_opt.take();

                // update repo size to remaining size
                repo_size = repo_size.saturating_sub(index);
                // update pct to be proportional to remainging size
                pct = repo_size / 100;

                // reset index to 0 for proper percent scaling
                index = 0;

                tracing::warn!(
                    "Caught up to previous migration's end. {repo_size} hashes will be migrated"
                );
            }
            continue;
        }

        let original_identifier = match repo.identifier(hash.as_ref().to_vec().into()).await {
            Ok(Some(identifier)) => identifier,
            Ok(None) => {
                tracing::warn!(
                    "Original File identifier for hash {} is missing, queue cleanup task",
                    hex::encode(&hash)
                );
                crate::queue::cleanup_hash(repo, hash).await?;
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        if let Some(identifier) = repo
            .motion_identifier(hash.as_ref().to_vec().into())
            .await?
        {
            if repo.get(STORE_MIGRATION_MOTION).await?.is_none() {
                match migrate_file(&from, &to, &identifier, skip_missing_files).await {
                    Ok(new_identifier) => {
                        migrate_details(repo, identifier, &new_identifier).await?;
                        repo.relate_motion_identifier(
                            hash.as_ref().to_vec().into(),
                            &new_identifier,
                        )
                        .await?;
                        repo.set(STORE_MIGRATION_MOTION, b"1".to_vec().into())
                            .await?;
                    }
                    Err(MigrateError::From(e)) if e.is_not_found() && skip_missing_files => {
                        tracing::warn!("Skipping motion file for hash {}", hex::encode(&hash));
                    }
                    Err(MigrateError::From(e)) => {
                        tracing::warn!("Error migrating motion file from old store");
                        return Err(e.into());
                    }
                    Err(MigrateError::To(e)) => {
                        tracing::warn!("Error migrating motion file to new store");
                        return Err(e.into());
                    }
                }
            }
        }

        let mut variant_progress_opt = repo.get(STORE_MIGRATION_VARIANT).await?;

        for (variant, identifier) in repo.variants(hash.as_ref().to_vec().into()).await? {
            if let Some(variant_progress) = &variant_progress_opt {
                if variant.as_bytes() == variant_progress.as_ref() {
                    variant_progress_opt.take();
                }
                continue;
            }

            match migrate_file(&from, &to, &identifier, skip_missing_files).await {
                Ok(new_identifier) => {
                    migrate_details(repo, identifier, &new_identifier).await?;
                    repo.remove_variant(hash.as_ref().to_vec().into(), variant.clone())
                        .await?;
                    repo.relate_variant_identifier(
                        hash.as_ref().to_vec().into(),
                        variant,
                        &new_identifier,
                    )
                    .await?;

                    repo.set(STORE_MIGRATION_VARIANT, new_identifier.to_bytes()?.into())
                        .await?;
                }
                Err(MigrateError::From(e)) if e.is_not_found() && skip_missing_files => {
                    tracing::warn!(
                        "Skipping variant {} for hash {}",
                        variant,
                        hex::encode(&hash)
                    );
                }
                Err(MigrateError::From(e)) => {
                    tracing::warn!("Error migrating variant file from old store");
                    return Err(e.into());
                }
                Err(MigrateError::To(e)) => {
                    tracing::warn!("Error migrating variant file to new store");
                    return Err(e.into());
                }
            }
        }

        match migrate_file(&from, &to, &original_identifier, skip_missing_files).await {
            Ok(new_identifier) => {
                migrate_details(repo, original_identifier, &new_identifier).await?;
                repo.relate_identifier(hash.as_ref().to_vec().into(), &new_identifier)
                    .await?;
            }
            Err(MigrateError::From(e)) if e.is_not_found() && skip_missing_files => {
                tracing::warn!("Skipping original file for hash {}", hex::encode(&hash));
            }
            Err(MigrateError::From(e)) => {
                tracing::warn!("Error migrating original file from old store");
                return Err(e.into());
            }
            Err(MigrateError::To(e)) => {
                tracing::warn!("Error migrating original file to new store");
                return Err(e.into());
            }
        }

        repo.set(STORE_MIGRATION_PROGRESS, hash.as_ref().to_vec().into())
            .await?;
        repo.remove(STORE_MIGRATION_VARIANT).await?;
        repo.remove(STORE_MIGRATION_MOTION).await?;

        if pct > 0 && index % pct == 0 {
            let percent = u32::try_from(index / pct).expect("values 0-100 are always in u32 range");
            if percent == 0 {
                continue;
            }

            let elapsed = now.elapsed();
            let estimated_duration_percent = elapsed / percent;
            let estimated_duration_remaining =
                (100u32.saturating_sub(percent)) * estimated_duration_percent;

            tracing::warn!("Migrated {percent}% of hashes ({index}/{repo_size} total hashes)");
            tracing::warn!("ETA: {estimated_duration_remaining:?} from now");
        }
    }

    // clean up the migration key to avoid interfering with future migrations
    repo.remove(STORE_MIGRATION_PROGRESS).await?;

    tracing::warn!("Migration completed successfully");

    Ok(())
}

async fn migrate_file<S1, S2>(
    from: &S1,
    to: &S2,
    identifier: &S1::Identifier,
    skip_missing_files: bool,
) -> Result<S2::Identifier, MigrateError>
where
    S1: Store,
    S2: Store,
{
    let mut failure_count = 0;

    loop {
        match do_migrate_file(from, to, identifier).await {
            Ok(identifier) => return Ok(identifier),
            Err(MigrateError::From(e)) if e.is_not_found() && skip_missing_files => {
                return Err(MigrateError::From(e));
            }
            Err(migrate_error) => {
                failure_count += 1;

                if failure_count > 10 {
                    tracing::error!("Error migrating file, not retrying");
                    return Err(migrate_error);
                } else {
                    tracing::warn!("Failed moving file. Retrying +{failure_count}");
                }

                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

#[derive(Debug)]
enum MigrateError {
    From(crate::store::StoreError),
    To(crate::store::StoreError),
}

async fn do_migrate_file<S1, S2>(
    from: &S1,
    to: &S2,
    identifier: &S1::Identifier,
) -> Result<S2::Identifier, MigrateError>
where
    S1: Store,
    S2: Store,
{
    let stream = from
        .to_stream(identifier, None, None)
        .await
        .map_err(MigrateError::From)?;

    let new_identifier = to.save_stream(stream).await.map_err(MigrateError::To)?;

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
