mod backgrounded;
mod bytes_stream;
mod concurrent_processor;
mod config;
mod details;
mod discover;
mod either;
mod error;
mod exiftool;
mod ffmpeg;
mod file;
mod formats;
mod generate;
mod ingest;
mod init_tracing;
mod magick;
mod middleware;
mod migrate_store;
mod process;
mod processor;
mod queue;
mod range;
mod repo;
mod repo_04;
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
use futures_util::{
    stream::{empty, once},
    Stream, StreamExt, TryStreamExt,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use middleware::Metrics;
use once_cell::sync::Lazy;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use rusty_s3::UrlStyle;
use std::{
    future::ready,
    path::Path,
    path::PathBuf,
    time::{Duration, SystemTime},
};
use tokio::sync::Semaphore;
use tracing_actix_web::TracingLogger;
use tracing_futures::Instrument;

use self::{
    backgrounded::Backgrounded,
    concurrent_processor::ProcessMap,
    config::{Configuration, Operation},
    details::Details,
    either::Either,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    ingest::Session,
    init_tracing::init_tracing,
    middleware::{Deadline, Internal},
    migrate_store::migrate_store,
    queue::queue_generate,
    repo::{
        sled::SledRepo, Alias, AliasAccessRepo, DeleteToken, FullRepo, HashRepo, IdentifierRepo,
        Repo, SettingsRepo, UploadId, UploadResult, VariantAccessRepo,
    },
    serde_str::Serde,
    store::{
        file_store::FileStore,
        object_store::{ObjectStore, ObjectStoreConfig},
        Identifier, Store,
    },
    stream::{StreamLimit, StreamTimeout},
};

pub use self::config::{ConfigSource, PictRsConfiguration};

const MEGABYTES: usize = 1024 * 1024;
const MINUTES: u32 = 60;
const HOURS: u32 = 60 * MINUTES;
const DAYS: u32 = 24 * HOURS;

const NOT_FOUND_KEY: &str = "404-alias";

static PROCESS_SEMAPHORE: Lazy<Semaphore> = Lazy::new(|| {
    tracing::trace_span!(parent: None, "Initialize semaphore")
        .in_scope(|| Semaphore::new(num_cpus::get().saturating_sub(1).max(1)))
});

async fn ensure_details<R: FullRepo, S: Store + 'static>(
    repo: &R,
    store: &S,
    config: &Configuration,
    alias: &Alias,
) -> Result<Details, Error> {
    let Some(identifier) = repo.identifier_from_alias::<S::Identifier>(alias).await? else {
        return Err(UploadError::MissingAlias.into());
    };

    let details = repo.details(&identifier).await?.and_then(|details| {
        if details.internal_format().is_some() {
            Some(details)
        } else {
            None
        }
    });

    if let Some(details) = details {
        tracing::debug!("details exist");
        Ok(details)
    } else {
        if config.server.read_only {
            return Err(UploadError::ReadOnly.into());
        }

        tracing::debug!("generating new details from {:?}", identifier);
        let new_details =
            Details::from_store(store, &identifier, config.media.process_timeout).await?;
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
        let config = req
            .app_data::<web::Data<Configuration>>()
            .expect("No configuration in request")
            .clone();

        Form::new()
            .max_files(config.server.max_file_count)
            .max_file_size(config.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let repo = repo.clone();
                    let store = store.clone();
                    let config = config.clone();

                    metrics::increment_counter!("pict-rs.files", "upload" => "inline");

                    let span = tracing::info_span!("file-upload", ?filename);

                    let stream = stream.map_err(Error::from);

                    Box::pin(
                        async move {
                            if config.server.read_only {
                                return Err(UploadError::ReadOnly.into());
                            }

                            ingest::ingest(&**repo, &**store, stream, None, &config.media).await
                        }
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
        let config = req
            .app_data::<web::Data<Configuration>>()
            .expect("No configuration in request")
            .clone();

        // Create a new Multipart Form validator for internal imports
        //
        // This form is expecting a single array field, 'images' with at most 10 files in it
        Form::new()
            .max_files(config.server.max_file_count)
            .max_file_size(config.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let repo = repo.clone();
                    let store = store.clone();
                    let config = config.clone();

                    metrics::increment_counter!("pict-rs.files", "import" => "inline");

                    let span = tracing::info_span!("file-import", ?filename);

                    let stream = stream.map_err(Error::from);

                    Box::pin(
                        async move {
                            if config.server.read_only {
                                return Err(UploadError::ReadOnly.into());
                            }

                            ingest::ingest(
                                &**repo,
                                &**store,
                                stream,
                                Some(Alias::from_existing(&filename)),
                                &config.media,
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
#[tracing::instrument(name = "Uploaded files", skip(value, repo, store, config))]
async fn upload<R: FullRepo, S: Store + 'static>(
    Multipart(Upload(value)): Multipart<Upload<R, S>>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    handle_upload(value, repo, store, config).await
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Imported files", skip(value, repo, store, config))]
async fn import<R: FullRepo, S: Store + 'static>(
    Multipart(Import(value)): Multipart<Import<R, S>>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    handle_upload(value, repo, store, config).await
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Uploaded files", skip(value, repo, store, config))]
async fn handle_upload<R: FullRepo, S: Store + 'static>(
    value: Value<Session<R, S>>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
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
            let delete_token = image.result.delete_token();

            let details = ensure_details(&repo, &store, &config, alias).await?;

            files.push(serde_json::json!({
                "file": alias.to_string(),
                "delete_token": delete_token.to_string(),
                "details": details,
            }));
        }
    }

    for image in images {
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
        let config = req
            .app_data::<web::Data<Configuration>>()
            .expect("No configuration in request")
            .clone();

        let read_only = config.server.read_only;

        Form::new()
            .max_files(config.server.max_file_count)
            .max_file_size(config.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let repo = (**repo).clone();
                    let store = (**store).clone();

                    metrics::increment_counter!("pict-rs.files", "upload" => "background");

                    let span = tracing::info_span!("file-proxy", ?filename);

                    let stream = stream.map_err(Error::from);

                    Box::pin(
                        async move {
                            if read_only {
                                return Err(UploadError::ReadOnly.into());
                            }

                            Backgrounded::proxy(repo, store, stream).await
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

        queue::queue_ingest(&repo, identifier, upload_id, None).await?;

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
    config: web::Data<Configuration>,
    query: web::Query<ClaimQuery>,
) -> Result<HttpResponse, Error> {
    let upload_id = Serde::into_inner(query.into_inner().upload_id);

    match actix_rt::time::timeout(Duration::from_secs(10), repo.wait(upload_id)).await {
        Ok(wait_res) => {
            let upload_result = wait_res?;
            repo.claim(upload_id).await?;
            metrics::increment_counter!("pict-rs.background.upload.claim");

            match upload_result {
                UploadResult::Success { alias, token } => {
                    let details = ensure_details(&repo, &store, &config, &alias).await?;

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

async fn ingest_inline<R: FullRepo, S: Store + 'static>(
    stream: impl Stream<Item = Result<web::Bytes, Error>> + Unpin + 'static,
    repo: &R,
    store: &S,
    config: &Configuration,
) -> Result<(Alias, DeleteToken, Details), Error> {
    let session = ingest::ingest(repo, store, stream, None, &config.media).await?;

    let alias = session.alias().expect("alias should exist").to_owned();

    let details = ensure_details(repo, store, config, &alias).await?;

    let delete_token = session.disarm();

    Ok((alias, delete_token, details))
}

/// download an image from a URL
#[tracing::instrument(name = "Downloading file", skip(client, repo, store, config))]
async fn download<R: FullRepo + 'static, S: Store + 'static>(
    client: web::Data<ClientWithMiddleware>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
    query: web::Query<UrlQuery>,
) -> Result<HttpResponse, Error> {
    let stream = download_stream(client, &query.url, &config).await?;

    if query.backgrounded {
        do_download_backgrounded(stream, repo, store).await
    } else {
        do_download_inline(stream, repo, store, config).await
    }
}

async fn download_stream(
    client: web::Data<ClientWithMiddleware>,
    url: &str,
    config: &Configuration,
) -> Result<impl Stream<Item = Result<web::Bytes, Error>> + Unpin + 'static, Error> {
    if config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let res = client.get(url).send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()).into());
    }

    let stream = res
        .bytes_stream()
        .map_err(Error::from)
        .limit((config.media.max_file_size * MEGABYTES) as u64);

    Ok(stream)
}

#[tracing::instrument(name = "Downloading file inline", skip(stream, repo, store, config))]
async fn do_download_inline<R: FullRepo, S: Store + 'static>(
    stream: impl Stream<Item = Result<web::Bytes, Error>> + Unpin + 'static,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    metrics::increment_counter!("pict-rs.files", "download" => "inline");

    let (alias, delete_token, details) = ingest_inline(stream, &repo, &store, &config).await?;

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
    metrics::increment_counter!("pict-rs.files", "download" => "background");

    let backgrounded = Backgrounded::proxy((**repo).clone(), (**store).clone(), stream).await?;

    let upload_id = backgrounded.upload_id().expect("Upload ID exists");
    let identifier = backgrounded
        .identifier()
        .expect("Identifier exists")
        .to_bytes()?;

    queue::queue_ingest(&repo, identifier, upload_id, None).await?;

    backgrounded.disarm();

    Ok(HttpResponse::Accepted().json(&serde_json::json!({
        "msg": "ok",
        "uploads": [{
            "upload_id": upload_id.to_string(),
        }]
    })))
}

/// Delete aliases and files
#[tracing::instrument(name = "Deleting file", skip(repo, config))]
async fn delete<R: FullRepo>(
    repo: web::Data<R>,
    config: web::Data<Configuration>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, Error> {
    if config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let (token, alias) = path_entries.into_inner();

    let token = DeleteToken::from_existing(&token);
    let alias = Alias::from_existing(&alias);

    queue::cleanup_alias(&repo, alias, token).await?;

    Ok(HttpResponse::NoContent().finish())
}

#[derive(Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
enum ProcessSource {
    Source { src: Serde<Alias> },
    Alias { alias: Serde<Alias> },
    Proxy { proxy: url::Url },
}

#[derive(Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct ProcessQuery {
    #[serde(flatten)]
    source: ProcessSource,

    #[serde(with = "tuple_vec_map", flatten)]
    operations: Vec<(String, String)>,
}

fn prepare_process(
    config: &Configuration,
    operations: Vec<(String, String)>,
    ext: &str,
) -> Result<(InputProcessableFormat, PathBuf, Vec<String>), Error> {
    let operations = operations
        .into_iter()
        .filter(|(k, _)| config.media.filters.contains(&k.to_lowercase()))
        .collect::<Vec<_>>();

    let format = ext
        .parse::<InputProcessableFormat>()
        .map_err(|_| UploadError::UnsupportedProcessExtension)?;

    let (thumbnail_path, thumbnail_args) =
        self::processor::build_chain(&operations, &format.to_string())?;

    Ok((format, thumbnail_path, thumbnail_args))
}

#[tracing::instrument(name = "Fetching derived details", skip(repo, config))]
async fn process_details<R: FullRepo, S: Store>(
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let alias = match source {
        ProcessSource::Alias { alias } | ProcessSource::Source { src: alias } => {
            Serde::into_inner(alias)
        }
        ProcessSource::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().json(&serde_json::json!({
                    "msg": "No images associated with provided proxy url"
                })));
            };
            alias
        }
    };

    let (_, thumbnail_path, _) = prepare_process(&config, operations, ext.as_str())?;

    let Some(hash) = repo.hash(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().json(&serde_json::json!({
            "msg": "No images associated with provided alias",
        })));
    };

    let thumbnail_string = thumbnail_path.to_string_lossy().to_string();

    if !config.server.read_only {
        VariantAccessRepo::accessed(&repo, hash.clone(), thumbnail_string.clone()).await?;
    }

    let identifier = repo
        .variant_identifier::<S::Identifier>(hash, thumbnail_string)
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
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    name = "Serving processed image",
    skip(repo, store, client, config, process_map)
)]
async fn process<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
    store: web::Data<S>,
    client: web::Data<ClientWithMiddleware>,
    config: web::Data<Configuration>,
    process_map: web::Data<ProcessMap>,
) -> Result<HttpResponse, Error> {
    let alias = match source {
        ProcessSource::Alias { alias } | ProcessSource::Source { src: alias } => {
            Serde::into_inner(alias)
        }
        ProcessSource::Proxy { proxy } => {
            let alias = if let Some(alias) = repo.related(proxy.clone()).await? {
                alias
            } else if !config.server.read_only {
                let stream = download_stream(client, proxy.as_str(), &config).await?;

                let (alias, _, _) = ingest_inline(stream, &repo, &store, &config).await?;

                repo.relate_url(proxy, alias.clone()).await?;

                alias
            } else {
                return Err(UploadError::ReadOnly.into());
            };

            if !config.server.read_only {
                AliasAccessRepo::accessed(&repo, alias.clone()).await?;
            }

            alias
        }
    };

    let (format, thumbnail_path, thumbnail_args) =
        prepare_process(&config, operations, ext.as_str())?;

    let path_string = thumbnail_path.to_string_lossy().to_string();

    let (hash, alias, not_found) = if let Some(hash) = repo.hash(&alias).await? {
        (hash, alias, false)
    } else {
        let Some((alias, hash)) = not_found_hash(&repo).await? else {
            return Ok(HttpResponse::NotFound().finish());
        };

        (hash, alias, true)
    };

    if !config.server.read_only {
        VariantAccessRepo::accessed(&repo, hash.clone(), path_string.clone()).await?;
    }

    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if let Some(identifier) = identifier_opt {
        let details = repo.details(&identifier).await?.and_then(|details| {
            if details.internal_format().is_some() {
                Some(details)
            } else {
                None
            }
        });

        let details = if let Some(details) = details {
            tracing::debug!("details exist");
            details
        } else {
            if config.server.read_only {
                return Err(UploadError::ReadOnly.into());
            }

            tracing::debug!("generating new details from {:?}", identifier);
            let new_details =
                Details::from_store(&store, &identifier, config.media.process_timeout).await?;
            tracing::debug!("storing details for {:?}", identifier);
            repo.relate_details(&identifier, &new_details).await?;
            tracing::debug!("stored");
            new_details
        };

        if let Some(public_url) = store.public_url(&identifier) {
            return Ok(HttpResponse::SeeOther()
                .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
                .finish());
        }

        return ranged_file_resp(&store, identifier, range, details, not_found).await;
    }

    if config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let original_details = ensure_details(&repo, &store, &config, &alias).await?;

    let (details, bytes) = generate::generate(
        &repo,
        &store,
        &process_map,
        format,
        alias,
        thumbnail_path,
        thumbnail_args,
        original_details.video_format(),
        None,
        &config.media,
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
        details.media_type(),
        7 * DAYS,
        details.system_time(),
    ))
}

#[tracing::instrument(name = "Serving processed image headers", skip(repo, store, config))]
async fn process_head<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let alias = match source {
        ProcessSource::Alias { alias } | ProcessSource::Source { src: alias } => {
            Serde::into_inner(alias)
        }
        ProcessSource::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

    let (_, thumbnail_path, _) = prepare_process(&config, operations, ext.as_str())?;

    let path_string = thumbnail_path.to_string_lossy().to_string();
    let Some(hash) = repo.hash(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().finish());
    };

    if !config.server.read_only {
        VariantAccessRepo::accessed(&repo, hash.clone(), path_string.clone()).await?;
    }

    let identifier_opt = repo
        .variant_identifier::<S::Identifier>(hash.clone(), path_string)
        .await?;

    if let Some(identifier) = identifier_opt {
        let details = repo.details(&identifier).await?.and_then(|details| {
            if details.internal_format().is_some() {
                Some(details)
            } else {
                None
            }
        });

        let details = if let Some(details) = details {
            tracing::debug!("details exist");
            details
        } else {
            if config.server.read_only {
                return Err(UploadError::ReadOnly.into());
            }

            tracing::debug!("generating new details from {:?}", identifier);
            let new_details =
                Details::from_store(&store, &identifier, config.media.process_timeout).await?;
            tracing::debug!("storing details for {:?}", identifier);
            repo.relate_details(&identifier, &new_details).await?;
            tracing::debug!("stored");
            new_details
        };

        if let Some(public_url) = store.public_url(&identifier) {
            return Ok(HttpResponse::SeeOther()
                .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
                .finish());
        }

        return ranged_file_head_resp(&store, identifier, range, details).await;
    }

    Ok(HttpResponse::NotFound().finish())
}

/// Process files
#[tracing::instrument(name = "Spawning image process", skip(repo))]
async fn process_backgrounded<R: FullRepo, S: Store>(
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    repo: web::Data<R>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let source = match source {
        ProcessSource::Alias { alias } | ProcessSource::Source { src: alias } => {
            Serde::into_inner(alias)
        }
        ProcessSource::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

    let (target_format, process_path, process_args) =
        prepare_process(&config, operations, ext.as_str())?;

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

    if config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    queue_generate(&repo, target_format, source, process_path, process_args).await?;

    Ok(HttpResponse::Accepted().finish())
}

/// Fetch file details
#[tracing::instrument(name = "Fetching query details", skip(repo, store, config))]
async fn details_query<R: FullRepo, S: Store + 'static>(
    web::Query(alias_query): web::Query<AliasQuery>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let alias = match alias_query {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().json(&serde_json::json!({
                    "msg": "Provided proxy URL has not been cached",
                })))
            };
            alias
        }
    };

    do_details(alias, repo, store, config).await
}

/// Fetch file details
#[tracing::instrument(name = "Fetching details", skip(repo, store, config))]
async fn details<R: FullRepo, S: Store + 'static>(
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    do_details(Serde::into_inner(alias.into_inner()), repo, store, config).await
}

async fn do_details<R: FullRepo, S: Store + 'static>(
    alias: Alias,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let details = ensure_details(&repo, &store, &config, &alias).await?;

    Ok(HttpResponse::Ok().json(&details))
}

/// Serve files based on alias query
#[tracing::instrument(name = "Serving file query", skip(repo, store, client, config))]
async fn serve_query<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(alias_query): web::Query<AliasQuery>,
    repo: web::Data<R>,
    store: web::Data<S>,
    client: web::Data<ClientWithMiddleware>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let alias = match alias_query {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { proxy } => {
            let alias = if let Some(alias) = repo.related(proxy.clone()).await? {
                alias
            } else if !config.server.read_only {
                let stream = download_stream(client, proxy.as_str(), &config).await?;

                let (alias, _, _) = ingest_inline(stream, &repo, &store, &config).await?;

                repo.relate_url(proxy, alias.clone()).await?;

                alias
            } else {
                return Err(UploadError::ReadOnly.into());
            };

            if !config.server.read_only {
                AliasAccessRepo::accessed(&repo, alias.clone()).await?;
            }

            alias
        }
    };

    do_serve(range, alias, repo, store, config).await
}

/// Serve files
#[tracing::instrument(name = "Serving file", skip(repo, store, config))]
async fn serve<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    do_serve(
        range,
        Serde::into_inner(alias.into_inner()),
        repo,
        store,
        config,
    )
    .await
}

async fn do_serve<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: Alias,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let (hash, alias, not_found) = if let Some(hash) = repo.hash(&alias).await? {
        (hash, alias, false)
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

    let details = ensure_details(&repo, &store, &config, &alias).await?;

    if let Some(public_url) = store.public_url(&identifier) {
        return Ok(HttpResponse::SeeOther()
            .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
            .finish());
    }

    ranged_file_resp(&store, identifier, range, details, not_found).await
}

#[tracing::instrument(name = "Serving query file headers", skip(repo, store, config))]
async fn serve_query_head<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(alias_query): web::Query<AliasQuery>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let alias = match alias_query {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

    do_serve_head(range, alias, repo, store, config).await
}

#[tracing::instrument(name = "Serving file headers", skip(repo, store, config))]
async fn serve_head<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: web::Path<Serde<Alias>>,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    do_serve_head(
        range,
        Serde::into_inner(alias.into_inner()),
        repo,
        store,
        config,
    )
    .await
}

async fn do_serve_head<R: FullRepo, S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: Alias,
    repo: web::Data<R>,
    store: web::Data<S>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    let Some(identifier) = repo.identifier_from_alias::<S::Identifier>(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().finish());
    };

    let details = ensure_details(&repo, &store, &config, &alias).await?;

    if let Some(public_url) = store.public_url(&identifier) {
        return Ok(HttpResponse::SeeOther()
            .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
            .finish());
    }

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
        details.media_type(),
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
        details.media_type(),
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

#[tracing::instrument(name = "Spawning variant cleanup", skip(repo, config))]
async fn clean_variants<R: FullRepo>(
    repo: web::Data<R>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    if config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    queue::cleanup_all_variants(&repo).await?;
    Ok(HttpResponse::NoContent().finish())
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum AliasQuery {
    Proxy { proxy: url::Url },
    Alias { alias: Serde<Alias> },
}

#[tracing::instrument(name = "Setting 404 Image", skip(repo, config))]
async fn set_not_found<R: FullRepo>(
    json: web::Json<AliasQuery>,
    repo: web::Data<R>,
    client: web::Data<ClientWithMiddleware>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    if config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let alias = match json.into_inner() {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { .. } => {
            return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "msg": "Cannot use proxied media as Not Found image",
            })));
        }
    };

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

#[tracing::instrument(name = "Purging file", skip(repo, config))]
async fn purge<R: FullRepo>(
    web::Query(alias_query): web::Query<AliasQuery>,
    repo: web::Data<R>,
    config: web::Data<Configuration>,
) -> Result<HttpResponse, Error> {
    if config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let alias = match alias_query {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

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
    web::Query(alias_query): web::Query<AliasQuery>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let alias = match alias_query {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

    let aliases = repo.aliases_from_alias(&alias).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>()
    })))
}

#[tracing::instrument(name = "Fetching identifier", skip(repo))]
async fn identifier<R: FullRepo, S: Store>(
    web::Query(alias_query): web::Query<AliasQuery>,
    repo: web::Data<R>,
) -> Result<HttpResponse, Error> {
    let alias = match alias_query {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { proxy } => {
            let Some(alias) = repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

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

fn build_client(config: &Configuration) -> Result<ClientWithMiddleware, Error> {
    let client = reqwest::Client::builder()
        .user_agent("pict-rs v0.5.0-main")
        .use_rustls_tls()
        .pool_max_idle_per_host(config.client.pool_size)
        .build()
        .map_err(UploadError::BuildClient)?;

    Ok(ClientBuilder::new(client)
        .with(TracingMiddleware::default())
        .build())
}

fn configure_endpoints<
    R: FullRepo + 'static,
    S: Store + 'static,
    F: Fn(&mut web::ServiceConfig),
>(
    config: &mut web::ServiceConfig,
    repo: R,
    store: S,
    configuration: Configuration,
    client: ClientWithMiddleware,
    extra_config: F,
) {
    config
        .app_data(web::Data::new(repo))
        .app_data(web::Data::new(store))
        .app_data(web::Data::new(client))
        .app_data(web::Data::new(configuration.clone()))
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
                    web::scope("/original")
                        .service(
                            web::resource("")
                                .route(web::get().to(serve_query::<R, S>))
                                .route(web::head().to(serve_query_head::<R, S>)),
                        )
                        .service(
                            web::resource("/{filename}")
                                .route(web::get().to(serve::<R, S>))
                                .route(web::head().to(serve_head::<R, S>)),
                        ),
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
                            web::scope("/original")
                                .service(
                                    web::resource("").route(web::get().to(details_query::<R, S>)),
                                )
                                .service(
                                    web::resource("/{filename}")
                                        .route(web::get().to(details::<R, S>)),
                                ),
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
                    configuration.server.api_key.as_ref().map(|s| s.to_owned()),
                ))
                .service(web::resource("/import").route(web::post().to(import::<R, S>)))
                .service(web::resource("/variants").route(web::delete().to(clean_variants::<R>)))
                .service(web::resource("/purge").route(web::post().to(purge::<R>)))
                .service(web::resource("/aliases").route(web::get().to(aliases::<R>)))
                .service(web::resource("/identifier").route(web::get().to(identifier::<R, S>)))
                .service(web::resource("/set_not_found").route(web::post().to(set_not_found::<R>)))
                .configure(extra_config),
        );
}

fn spawn_cleanup<R>(repo: R, config: &Configuration)
where
    R: FullRepo + 'static,
{
    if config.server.read_only {
        return;
    }

    tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
        actix_rt::spawn(async move {
            let mut interval = actix_rt::time::interval(Duration::from_secs(30));

            loop {
                interval.tick().await;

                if let Err(e) = queue::cleanup_outdated_variants(&repo).await {
                    tracing::warn!(
                        "Failed to spawn cleanup for outdated variants:{}",
                        format!("\n{e}\n{e:?}")
                    );
                }

                if let Err(e) = queue::cleanup_outdated_proxies(&repo).await {
                    tracing::warn!(
                        "Failed to spawn cleanup for outdated proxies:{}",
                        format!("\n{e}\n{e:?}")
                    );
                }
            }
        });
    })
}

fn spawn_workers<R, S>(repo: R, store: S, config: Configuration, process_map: ProcessMap)
where
    R: FullRepo + 'static,
    S: Store + 'static,
{
    tracing::trace_span!(parent: None, "Spawn task").in_scope(|| {
        actix_rt::spawn(queue::process_cleanup(
            repo.clone(),
            store.clone(),
            config.clone(),
        ))
    });
    tracing::trace_span!(parent: None, "Spawn task")
        .in_scope(|| actix_rt::spawn(queue::process_images(repo, store, process_map, config)));
}

async fn launch_file_store<R: FullRepo + 'static, F: Fn(&mut web::ServiceConfig) + Send + Clone>(
    repo: R,
    store: FileStore,
    client: ClientWithMiddleware,
    config: Configuration,
    extra_config: F,
) -> std::io::Result<()> {
    let process_map = ProcessMap::new();

    let address = config.server.address;

    spawn_cleanup(repo.clone(), &config);

    HttpServer::new(move || {
        let client = client.clone();
        let store = store.clone();
        let repo = repo.clone();
        let config = config.clone();
        let extra_config = extra_config.clone();

        spawn_workers(
            repo.clone(),
            store.clone(),
            config.clone(),
            process_map.clone(),
        );

        App::new()
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .wrap(Metrics)
            .app_data(web::Data::new(process_map.clone()))
            .configure(move |sc| configure_endpoints(sc, repo, store, config, client, extra_config))
    })
    .bind(address)?
    .run()
    .await
}

async fn launch_object_store<
    R: FullRepo + 'static,
    F: Fn(&mut web::ServiceConfig) + Send + Clone,
>(
    repo: R,
    store_config: ObjectStoreConfig,
    client: ClientWithMiddleware,
    config: Configuration,
    extra_config: F,
) -> std::io::Result<()> {
    let process_map = ProcessMap::new();

    let address = config.server.address;

    spawn_cleanup(repo.clone(), &config);

    HttpServer::new(move || {
        let client = client.clone();
        let store = store_config.clone().build(client.clone());
        let repo = repo.clone();
        let config = config.clone();
        let extra_config = extra_config.clone();

        spawn_workers(
            repo.clone(),
            store.clone(),
            config.clone(),
            process_map.clone(),
        );

        App::new()
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .wrap(Metrics)
            .app_data(web::Data::new(process_map.clone()))
            .configure(move |sc| configure_endpoints(sc, repo, store, config, client, extra_config))
    })
    .bind(address)?
    .run()
    .await
}

async fn migrate_inner<S1>(
    repo: Repo,
    client: ClientWithMiddleware,
    from: S1,
    to: config::primitives::Store,
    skip_missing_files: bool,
    timeout: u64,
) -> color_eyre::Result<()>
where
    S1: Store + 'static,
{
    match to {
        config::primitives::Store::Filesystem(config::Filesystem { path }) => {
            let to = FileStore::build(path.clone(), repo.clone()).await?;

            match repo {
                Repo::Sled(repo) => {
                    migrate_store(repo, from, to, skip_missing_files, timeout).await?
                }
            }
        }
        config::primitives::Store::ObjectStorage(config::primitives::ObjectStorage {
            endpoint,
            bucket_name,
            use_path_style,
            region,
            access_key,
            secret_key,
            session_token,
            signature_duration,
            client_timeout,
            public_endpoint,
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
                signature_duration.unwrap_or(15),
                client_timeout.unwrap_or(30),
                public_endpoint,
                repo.clone(),
            )
            .await?
            .build(client);

            match repo {
                Repo::Sled(repo) => {
                    migrate_store(repo, from, to, skip_missing_files, timeout).await?
                }
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
    pub fn init<Q: AsRef<Path>>(
        self,
        save_to: Option<Q>,
    ) -> color_eyre::Result<PictRsConfiguration> {
        config::configure_without_clap(self, save_to)
    }
}

async fn export_handler(repo: web::Data<SledRepo>) -> Result<HttpResponse, Error> {
    repo.export().await?;

    Ok(HttpResponse::Created().json(&serde_json::json!({
        "msg": "ok"
    })))
}

fn sled_extra_config(sc: &mut web::ServiceConfig) {
    sc.service(web::resource("/export").route(web::post().to(export_handler)));
}

impl PictRsConfiguration {
    /// Build the pict-rs configuration from commandline arguments
    ///
    /// This is probably not useful for 3rd party applications that handle their own commandline
    pub fn build_default() -> color_eyre::Result<Self> {
        config::configure()
    }

    /// Install the default pict-rs tracer
    ///
    /// This is probably not useful for 3rd party applications that install their own tracing
    /// subscribers.
    pub fn install_tracing(self) -> color_eyre::Result<Self> {
        init_tracing(&self.config.tracing)?;
        Ok(self)
    }

    pub fn install_metrics(self) -> color_eyre::Result<Self> {
        if let Some(addr) = self.config.metrics.prometheus_address {
            PrometheusBuilder::new()
                .with_http_listener(addr)
                .install()?;
        }

        Ok(self)
    }

    /// Run the pict-rs application
    ///
    /// This must be called after `init_config`, or else the default configuration builder will run and
    /// fail.
    pub async fn run(self) -> color_eyre::Result<()> {
        let PictRsConfiguration { config, operation } = self;

        let repo = Repo::open(config.repo.clone())?;
        let client = build_client(&config)?;

        match operation {
            Operation::Run => (),
            Operation::MigrateStore {
                skip_missing_files,
                from,
                to,
            } => {
                match from {
                    config::primitives::Store::Filesystem(config::Filesystem { path }) => {
                        let from = FileStore::build(path.clone(), repo.clone()).await?;
                        migrate_inner(
                            repo,
                            client,
                            from,
                            to,
                            skip_missing_files,
                            config.media.process_timeout,
                        )
                        .await?;
                    }
                    config::primitives::Store::ObjectStorage(
                        config::primitives::ObjectStorage {
                            endpoint,
                            bucket_name,
                            use_path_style,
                            region,
                            access_key,
                            secret_key,
                            session_token,
                            signature_duration,
                            client_timeout,
                            public_endpoint,
                        },
                    ) => {
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
                            signature_duration.unwrap_or(15),
                            client_timeout.unwrap_or(30),
                            public_endpoint,
                            repo.clone(),
                        )
                        .await?
                        .build(client.clone());

                        migrate_inner(
                            repo,
                            client,
                            from,
                            to,
                            skip_missing_files,
                            config.media.process_timeout,
                        )
                        .await?;
                    }
                }

                return Ok(());
            }
        }

        if config.server.read_only {
            tracing::warn!("Launching in READ ONLY mode");
        }

        match config.store.clone() {
            config::Store::Filesystem(config::Filesystem { path }) => {
                let store = FileStore::build(path, repo.clone()).await?;
                match repo {
                    Repo::Sled(sled_repo) => {
                        sled_repo
                            .mark_accessed::<<FileStore as Store>::Identifier>()
                            .await?;

                        launch_file_store(sled_repo, store, client, config, sled_extra_config)
                            .await?;
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
                signature_duration,
                client_timeout,
                public_endpoint,
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
                    signature_duration,
                    client_timeout,
                    public_endpoint,
                    repo.clone(),
                )
                .await?;

                match repo {
                    Repo::Sled(sled_repo) => {
                        sled_repo
                            .mark_accessed::<<ObjectStore as Store>::Identifier>()
                            .await?;

                        launch_object_store(sled_repo, store, client, config, sled_extra_config)
                            .await?;
                    }
                }
            }
        }

        self::tmp_file::remove_tmp_dir().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn source() {
        let query = super::ProcessQuery {
            source: super::ProcessSource::Source {
                src: super::Serde::new(super::Alias::from_existing("example.png")),
            },
            operations: vec![("resize".into(), "200".into())],
        };
        let encoded = serde_urlencoded::to_string(&query).expect("Encoded");
        let new_query: super::ProcessQuery = serde_urlencoded::from_str(&encoded).expect("Decoded");
        // Don't compare entire query - "src" gets deserialized twice
        assert_eq!(new_query.source, query.source);

        assert!(new_query
            .operations
            .contains(&("resize".into(), "200".into())));
    }

    #[test]
    fn alias() {
        let query = super::ProcessQuery {
            source: super::ProcessSource::Alias {
                alias: super::Serde::new(super::Alias::from_existing("example.png")),
            },
            operations: vec![("resize".into(), "200".into())],
        };
        let encoded = serde_urlencoded::to_string(&query).expect("Encoded");
        let new_query: super::ProcessQuery = serde_urlencoded::from_str(&encoded).expect("Decoded");
        // Don't compare entire query - "alias" gets deserialized twice
        assert_eq!(new_query.source, query.source);

        assert!(new_query
            .operations
            .contains(&("resize".into(), "200".into())));
    }

    #[test]
    fn url() {
        let query = super::ProcessQuery {
            source: super::ProcessSource::Proxy {
                proxy: "http://example.com/image.png".parse().expect("valid url"),
            },
            operations: vec![("resize".into(), "200".into())],
        };
        let encoded = serde_urlencoded::to_string(&query).expect("Encoded");
        let new_query: super::ProcessQuery = serde_urlencoded::from_str(&encoded).expect("Decoded");
        // Don't compare entire query - "proxy" gets deserialized twice
        assert_eq!(new_query.source, query.source);

        assert!(new_query
            .operations
            .contains(&("resize".into(), "200".into())));
    }
}
