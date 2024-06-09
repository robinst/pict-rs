mod backgrounded;
mod blurhash;
mod bytes_stream;
mod config;
mod details;
mod discover;
mod either;
mod error;
mod error_code;
mod exiftool;
mod ffmpeg;
mod file;
mod file_path;
mod formats;
mod future;
mod generate;
mod http1;
mod ingest;
mod init_metrics;
mod init_tracing;
mod magick;
mod middleware;
mod migrate_store;
mod process;
mod processor;
mod queue;
mod range;
mod read;
mod repo;
mod repo_04;
mod serde_str;
mod state;
mod store;
mod stream;
mod sync;
mod tls;
mod tmp_file;

use actix_form_data::{Field, Form, FormData, Multipart, Value};
use actix_web::{
    guard,
    http::header::{CacheControl, CacheDirective, LastModified, Range, ACCEPT_RANGES},
    web, App, HttpRequest, HttpResponse, HttpResponseBuilder, HttpServer,
};
use futures_core::Stream;
use metrics_exporter_prometheus::PrometheusBuilder;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use rustls_channel_resolver::ChannelSender;
use rusty_s3::UrlStyle;
use std::{
    marker::PhantomData,
    path::Path,
    rc::Rc,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime},
};
use streem::IntoStreamer;
use tokio::sync::Semaphore;
use tracing::Instrument;
use tracing_actix_web::TracingLogger;

use self::{
    backgrounded::Backgrounded,
    config::{Configuration, Operation},
    details::{ApiDetails, Details, HumanDate},
    either::Either,
    error::{Error, UploadError},
    formats::InputProcessableFormat,
    future::{WithPollTimer, WithTimeout},
    ingest::Session,
    init_tracing::init_tracing,
    magick::ArcPolicyDir,
    middleware::{Deadline, Internal, Log, Metrics, Payload},
    migrate_store::migrate_store,
    queue::queue_generate,
    repo::{
        sled::SledRepo, Alias, ArcRepo, DeleteToken, Hash, ProxyAlreadyExists, Repo, UploadId,
        UploadResult,
    },
    serde_str::Serde,
    state::State,
    store::{file_store::FileStore, object_store::ObjectStore, Store},
    stream::empty,
    sync::DropHandle,
    tls::Tls,
    tmp_file::{ArcTmpDir, TmpDir},
};

pub use self::config::{ConfigSource, PictRsConfiguration};

const MEGABYTES: usize = 1024 * 1024;
const MINUTES: u32 = 60;
const HOURS: u32 = 60 * MINUTES;
const DAYS: u32 = 24 * HOURS;

const NOT_FOUND_KEY: &str = "404-alias";

static PROCESS_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn process_semaphore() -> &'static Semaphore {
    PROCESS_SEMAPHORE.get_or_init(|| {
        let permits = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .saturating_sub(1)
            .max(1);

        crate::sync::bare_semaphore(permits)
    })
}

async fn ensure_details<S: Store + 'static>(
    state: &State<S>,
    alias: &Alias,
) -> Result<Details, Error> {
    let Some(identifier) = state.repo.identifier_from_alias(alias).await? else {
        return Err(UploadError::MissingAlias.into());
    };

    ensure_details_identifier(state, &identifier).await
}

#[tracing::instrument(skip(state))]
async fn ensure_details_identifier<S: Store + 'static>(
    state: &State<S>,
    identifier: &Arc<str>,
) -> Result<Details, Error> {
    let details = state.repo.details(identifier).await?;

    if let Some(details) = details {
        Ok(details)
    } else {
        if state.config.server.read_only {
            return Err(UploadError::ReadOnly.into());
        } else if state.config.server.danger_dummy_mode {
            return Ok(Details::danger_dummy(formats::InternalFormat::Image(
                formats::ImageFormat::Png,
            )));
        }

        let bytes_stream = state.store.to_bytes(identifier, None, None).await?;
        let new_details = Details::from_bytes_stream(state, bytes_stream).await?;
        state.repo.relate_details(identifier, &new_details).await?;
        Ok(new_details)
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(default)]
struct UploadLimits {
    max_width: Option<Serde<u16>>,
    max_height: Option<Serde<u16>>,
    max_area: Option<Serde<u32>>,
    max_frame_count: Option<Serde<u32>>,
    max_file_size: Option<Serde<usize>>,
    allow_image: Serde<bool>,
    allow_animation: Serde<bool>,
    allow_video: Serde<bool>,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            max_area: None,
            max_frame_count: None,
            max_file_size: None,
            allow_image: Serde::new(true),
            allow_animation: Serde::new(true),
            allow_video: Serde::new(true),
        }
    }
}

#[derive(Clone, Default, Debug, serde::Deserialize, serde::Serialize)]
struct UploadQuery {
    #[serde(flatten)]
    limits: UploadLimits,

    #[serde(with = "tuple_vec_map", flatten)]
    operations: Vec<(String, String)>,
}

struct Upload<S>(Value<Session>, PhantomData<S>);

impl<S: Store + 'static> FormData for Upload<S> {
    type Item = Session;
    type Error = Error;

    fn form(req: &HttpRequest) -> Result<Form<Self::Item, Self::Error>, Self::Error> {
        let state = req
            .app_data::<web::Data<State<S>>>()
            .expect("No state in request")
            .clone();

        let web::Query(upload_query) = web::Query::<UploadQuery>::from_query(req.query_string())
            .map_err(UploadError::InvalidQuery)?;

        let upload_query = Rc::new(upload_query);

        // Create a new Multipart Form validator
        //
        // This form is expecting a single array field, 'images' with at most 10 files in it
        Ok(Form::new()
            .max_files(state.config.server.max_file_count)
            .max_file_size(state.config.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let state = state.clone();
                    let upload_query = upload_query.clone();

                    metrics::counter!(crate::init_metrics::FILES, "upload" => "inline")
                        .increment(1);

                    let span = tracing::info_span!("file-upload", ?filename);

                    Box::pin(
                        async move {
                            if state.config.server.read_only {
                                return Err(UploadError::ReadOnly.into());
                            }

                            let stream = crate::stream::from_err(stream);

                            ingest::ingest(&state, stream, None, &upload_query).await
                        }
                        .with_poll_timer("file-upload")
                        .instrument(span),
                    )
                })),
            ))
    }

    fn extract(value: Value<Self::Item>) -> Result<Self, Self::Error> {
        Ok(Upload(value, PhantomData))
    }
}

struct Import<S: Store + 'static>(Value<Session>, PhantomData<S>);

impl<S: Store + 'static> FormData for Import<S> {
    type Item = Session;
    type Error = Error;

    fn form(req: &actix_web::HttpRequest) -> Result<Form<Self::Item, Self::Error>, Self::Error> {
        let state = req
            .app_data::<web::Data<State<S>>>()
            .expect("No state in request")
            .clone();

        let web::Query(upload_query) = web::Query::<UploadQuery>::from_query(req.query_string())
            .map_err(UploadError::InvalidQuery)?;

        let upload_query = Rc::new(upload_query);

        // Create a new Multipart Form validator for internal imports
        //
        // This form is expecting a single array field, 'images' with at most 10 files in it
        Ok(Form::new()
            .max_files(state.config.server.max_file_count)
            .max_file_size(state.config.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let state = state.clone();
                    let upload_query = upload_query.clone();

                    metrics::counter!(crate::init_metrics::FILES, "import" => "inline")
                        .increment(1);

                    let span = tracing::info_span!("file-import", ?filename);

                    Box::pin(
                        async move {
                            if state.config.server.read_only {
                                return Err(UploadError::ReadOnly.into());
                            }

                            let stream = crate::stream::from_err(stream);

                            ingest::ingest(
                                &state,
                                stream,
                                Some(Alias::from_existing(&filename)),
                                &upload_query,
                            )
                            .await
                        }
                        .with_poll_timer("file-import")
                        .instrument(span),
                    )
                })),
            ))
    }

    fn extract(value: Value<Self::Item>) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Ok(Import(value, PhantomData))
    }
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Uploaded files", skip(value, state))]
async fn upload<S: Store + 'static>(
    Multipart(Upload(value, _)): Multipart<Upload<S>>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    handle_upload(value, state).await
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Imported files", skip(value, state))]
async fn import<S: Store + 'static>(
    Multipart(Import(value, _)): Multipart<Import<S>>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    handle_upload(value, state).await
}

/// Handle responding to successful uploads
#[tracing::instrument(name = "Uploaded files", skip(value, state))]
async fn handle_upload<S: Store + 'static>(
    value: Value<Session>,
    state: web::Data<State<S>>,
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

            let details = ensure_details(&state, alias).await?;

            files.push(serde_json::json!({
                "file": alias.to_string(),
                "delete_token": delete_token.to_string(),
                "details": details.into_api_details(),
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

struct BackgroundedUpload<S: Store + 'static>(Value<Backgrounded>, PhantomData<S>);

impl<S: Store + 'static> FormData for BackgroundedUpload<S> {
    type Item = Backgrounded;
    type Error = Error;

    fn form(req: &actix_web::HttpRequest) -> Result<Form<Self::Item, Self::Error>, Self::Error> {
        let state = req
            .app_data::<web::Data<State<S>>>()
            .expect("No state in request")
            .clone();

        // Create a new Multipart Form validator for backgrounded uploads
        //
        // This form is expecting a single array field, 'images' with at most 10 files in it
        Ok(Form::new()
            .max_files(state.config.server.max_file_count)
            .max_file_size(state.config.media.max_file_size * MEGABYTES)
            .transform_error(transform_error)
            .field(
                "images",
                Field::array(Field::file(move |filename, _, stream| {
                    let state = state.clone();

                    metrics::counter!(crate::init_metrics::FILES, "upload" => "background")
                        .increment(1);

                    let span = tracing::info_span!("file-proxy", ?filename);

                    Box::pin(
                        async move {
                            if state.config.server.read_only {
                                return Err(UploadError::ReadOnly.into());
                            }

                            let stream = crate::stream::from_err(stream);

                            Backgrounded::proxy(&state, stream).await
                        }
                        .with_poll_timer("file-proxy")
                        .instrument(span),
                    )
                })),
            ))
    }

    fn extract(value: Value<Self::Item>) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Ok(BackgroundedUpload(value, PhantomData))
    }
}

#[tracing::instrument(name = "Uploaded files", skip(value, state))]
async fn upload_backgrounded<S: Store>(
    Multipart(BackgroundedUpload(value, _)): Multipart<BackgroundedUpload<S>>,
    state: web::Data<State<S>>,
    upload_query: web::Query<UploadQuery>,
) -> Result<HttpResponse, Error> {
    let upload_query = upload_query.into_inner();

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
        let identifier = image.result.identifier().expect("Identifier exists");

        queue::queue_ingest(
            &state.repo,
            identifier,
            upload_id,
            None,
            upload_query.clone(),
        )
        .await?;

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
async fn claim_upload<S: Store + 'static>(
    query: web::Query<ClaimQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let upload_id = Serde::into_inner(query.into_inner().upload_id);

    match state
        .repo
        .wait(upload_id)
        .with_timeout(Duration::from_secs(10))
        .await
    {
        Ok(wait_res) => {
            let upload_result = wait_res?;
            state.repo.claim(upload_id).await?;
            metrics::counter!(crate::init_metrics::BACKGROUND_UPLOAD_CLAIM).increment(1);

            match upload_result {
                UploadResult::Success { alias, token } => {
                    let details = ensure_details(&state, &alias).await?;

                    Ok(HttpResponse::Ok().json(&serde_json::json!({
                        "msg": "ok",
                        "files": [{
                            "file": alias.to_string(),
                            "delete_token": token.to_string(),
                            "details": details.into_api_details(),
                        }]
                    })))
                }
                UploadResult::Failure { message, code } => Ok(HttpResponse::UnprocessableEntity()
                    .json(&serde_json::json!({
                        "msg": message,
                        "code": code,
                    }))),
            }
        }
        Err(_) => Ok(HttpResponse::NoContent().finish()),
    }
}

#[derive(Debug, serde::Deserialize)]
struct UrlQuery {
    url: String,

    #[serde(default)]
    backgrounded: Serde<bool>,
}

#[derive(Debug, serde::Deserialize)]
struct DownloadQuery {
    #[serde(flatten)]
    url_query: UrlQuery,

    #[serde(flatten)]
    upload_query: UploadQuery,
}

async fn ingest_inline<S: Store + 'static>(
    stream: impl Stream<Item = Result<web::Bytes, Error>>,
    state: &State<S>,
    upload_query: &UploadQuery,
) -> Result<(Alias, DeleteToken, Details), Error> {
    let session = ingest::ingest(state, stream, None, upload_query).await?;

    let alias = session.alias().expect("alias should exist").to_owned();

    let details = ensure_details(state, &alias).await?;

    let delete_token = session.disarm();

    Ok((alias, delete_token, details))
}

/// download an image from a URL
#[tracing::instrument(name = "Downloading file", skip(state))]
async fn download<S: Store + 'static>(
    download_query: web::Query<DownloadQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let DownloadQuery {
        url_query,
        upload_query,
    } = download_query.into_inner();

    let stream = download_stream(&url_query.url, &state).await?;

    if *url_query.backgrounded {
        do_download_backgrounded(stream, state, upload_query).await
    } else {
        do_download_inline(stream, &state, &upload_query).await
    }
}

async fn download_stream<S>(
    url: &str,
    state: &State<S>,
) -> Result<impl Stream<Item = Result<web::Bytes, Error>> + 'static, Error> {
    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let res = state.client.get(url).send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(http1::to_actix_status(res.status())).into());
    }

    let stream = crate::stream::limit(
        state.config.media.max_file_size * MEGABYTES,
        crate::stream::from_err(res.bytes_stream()),
    );

    Ok(stream)
}

#[tracing::instrument(name = "Downloading file inline", skip(stream, state))]
async fn do_download_inline<S: Store + 'static>(
    stream: impl Stream<Item = Result<web::Bytes, Error>>,
    state: &State<S>,
    upload_query: &UploadQuery,
) -> Result<HttpResponse, Error> {
    metrics::counter!(crate::init_metrics::FILES, "download" => "inline").increment(1);

    let (alias, delete_token, details) = ingest_inline(stream, state, upload_query).await?;

    Ok(HttpResponse::Created().json(&serde_json::json!({
        "msg": "ok",
        "files": [{
            "file": alias.to_string(),
            "delete_token": delete_token.to_string(),
            "details": details.into_api_details(),
        }]
    })))
}

#[tracing::instrument(name = "Downloading file in background", skip(stream, state))]
async fn do_download_backgrounded<S: Store + 'static>(
    stream: impl Stream<Item = Result<web::Bytes, Error>>,
    state: web::Data<State<S>>,
    upload_query: UploadQuery,
) -> Result<HttpResponse, Error> {
    metrics::counter!(crate::init_metrics::FILES, "download" => "background").increment(1);

    let backgrounded = Backgrounded::proxy(&state, stream).await?;

    let upload_id = backgrounded.upload_id().expect("Upload ID exists");
    let identifier = backgrounded.identifier().expect("Identifier exists");

    queue::queue_ingest(&state.repo, identifier, upload_id, None, upload_query).await?;

    backgrounded.disarm();

    Ok(HttpResponse::Accepted().json(&serde_json::json!({
        "msg": "ok",
        "uploads": [{
            "upload_id": upload_id.to_string(),
        }]
    })))
}

#[derive(Debug, serde::Deserialize)]
struct PageQuery {
    slug: Option<String>,
    timestamp: Option<HumanDate>,
    limit: Option<usize>,
}

#[derive(serde::Serialize)]
struct PageJson {
    limit: usize,

    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    prev: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,

    hashes: Vec<HashJson>,
}

#[derive(serde::Serialize)]
struct HashJson {
    hex: String,
    aliases: Vec<String>,
    details: Option<ApiDetails>,
}

/// Get a page of hashes
#[tracing::instrument(name = "Hash Page", skip(state))]
async fn page<S>(
    state: web::Data<State<S>>,
    web::Query(PageQuery {
        slug,
        timestamp,
        limit,
    }): web::Query<PageQuery>,
) -> Result<HttpResponse, Error> {
    let limit = limit.unwrap_or(20);

    let page = if let Some(timestamp) = timestamp {
        state
            .repo
            .hash_page_by_date(timestamp.timestamp, limit)
            .await?
    } else {
        state.repo.hash_page(slug, limit).await?
    };

    let mut hashes = Vec::with_capacity(page.hashes.len());

    for hash in &page.hashes {
        let hex = hash.to_hex();
        let aliases = state
            .repo
            .aliases_for_hash(hash.clone())
            .await?
            .into_iter()
            .map(|a| a.to_string())
            .collect();

        let identifier = state.repo.identifier(hash.clone()).await?;
        let details = if let Some(identifier) = identifier {
            state
                .repo
                .details(&identifier)
                .await?
                .map(|d| d.into_api_details())
        } else {
            None
        };

        hashes.push(HashJson {
            hex,
            aliases,
            details,
        });
    }

    let page = PageJson {
        limit: page.limit,
        current: page.current(),
        prev: page.prev(),
        next: page.next(),
        hashes,
    };

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "msg": "ok",
        "page": page,
    })))
}

/// Delete aliases and files
#[tracing::instrument(name = "Deleting file", skip(state))]
async fn delete<S>(
    state: web::Data<State<S>>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, Error> {
    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let (token, alias) = path_entries.into_inner();

    let token = DeleteToken::from_existing(&token);
    let alias = Alias::from_existing(&alias);

    // delete alias inline
    queue::cleanup::alias(&state.repo, alias, token).await?;

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
) -> Result<(InputProcessableFormat, String, Vec<String>), Error> {
    let operations = operations
        .into_iter()
        .filter(|(k, _)| config.media.filters.contains(&k.to_lowercase()))
        .collect::<Vec<_>>();

    let format = ext
        .parse::<InputProcessableFormat>()
        .map_err(|_| UploadError::UnsupportedProcessExtension)?;

    let (variant, variant_args) = self::processor::build_chain(&operations, &format.to_string())?;

    Ok((format, variant, variant_args))
}

#[tracing::instrument(name = "Fetching derived details", skip(state))]
async fn process_details<S: Store>(
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = alias_from_query(source.into(), &state).await?;

    let (_, variant, _) = prepare_process(&state.config, operations, ext.as_str())?;

    let hash = state
        .repo
        .hash(&alias)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    if !state.config.server.read_only {
        state
            .repo
            .accessed_variant(hash.clone(), variant.clone())
            .await?;
    }

    let identifier = state
        .repo
        .variant_identifier(hash, variant)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    let details = state.repo.details(&identifier).await?;

    let details = details.ok_or(UploadError::NoFiles)?;

    Ok(HttpResponse::Ok().json(&details.into_api_details()))
}

async fn not_found_hash(repo: &ArcRepo) -> Result<Option<(Alias, Hash)>, Error> {
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
#[tracing::instrument(name = "Serving processed image", skip(state))]
async fn process<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = proxy_alias_from_query(source.into(), &state).await?;

    let (format, variant, variant_args) = prepare_process(&state.config, operations, ext.as_str())?;

    let (hash, alias, not_found) = if let Some(hash) = state.repo.hash(&alias).await? {
        (hash, alias, false)
    } else {
        let Some((alias, hash)) = not_found_hash(&state.repo).await? else {
            return Ok(HttpResponse::NotFound().finish());
        };

        (hash, alias, true)
    };

    if !state.config.server.read_only {
        state
            .repo
            .accessed_variant(hash.clone(), variant.clone())
            .await?;
    }

    let identifier_opt = state
        .repo
        .variant_identifier(hash.clone(), variant.clone())
        .await?;

    let (details, identifier) = if let Some(identifier) = identifier_opt {
        let details = ensure_details_identifier(&state, &identifier).await?;

        (details, identifier)
    } else {
        if state.config.server.read_only {
            return Err(UploadError::ReadOnly.into());
        }

        queue_generate(&state.repo, format, alias, variant.clone(), variant_args).await?;

        let mut attempts = 0;
        loop {
            if attempts > 6 {
                return Err(UploadError::ProcessTimeout.into());
            }

            let entry = state
                .repo
                .variant_waiter(hash.clone(), variant.clone())
                .await?;

            let opt = generate::wait_timeout(
                hash.clone(),
                variant.clone(),
                entry,
                &state,
                Duration::from_secs(5),
            )
            .await?;

            if let Some(tuple) = opt {
                break tuple;
            }

            attempts += 1;
        }
    };

    if let Some(public_url) = state.store.public_url(&identifier) {
        return Ok(HttpResponse::SeeOther()
            .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
            .finish());
    }

    ranged_file_resp(&state.store, identifier, range, details, not_found).await
}

#[tracing::instrument(name = "Serving processed image headers", skip(state))]
async fn process_head<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = match source {
        ProcessSource::Alias { alias } | ProcessSource::Source { src: alias } => {
            Serde::into_inner(alias)
        }
        ProcessSource::Proxy { proxy } => {
            let Some(alias) = state.repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

    let (_, variant, _) = prepare_process(&state.config, operations, ext.as_str())?;

    let Some(hash) = state.repo.hash(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().finish());
    };

    if !state.config.server.read_only {
        state
            .repo
            .accessed_variant(hash.clone(), variant.clone())
            .await?;
    }

    let identifier_opt = state.repo.variant_identifier(hash.clone(), variant).await?;

    if let Some(identifier) = identifier_opt {
        let details = ensure_details_identifier(&state, &identifier).await?;

        if let Some(public_url) = state.store.public_url(&identifier) {
            return Ok(HttpResponse::SeeOther()
                .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
                .finish());
        }

        return ranged_file_head_resp(&state.store, identifier, range, details).await;
    }

    Ok(HttpResponse::NotFound().finish())
}

/// Process files
#[tracing::instrument(name = "Spawning image process", skip(state))]
async fn process_backgrounded<S: Store + 'static>(
    web::Query(ProcessQuery { source, operations }): web::Query<ProcessQuery>,
    ext: web::Path<String>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let source = match source {
        ProcessSource::Alias { alias } | ProcessSource::Source { src: alias } => {
            Serde::into_inner(alias)
        }
        ProcessSource::Proxy { proxy } => {
            let Some(alias) = state.repo.related(proxy).await? else {
                return Ok(HttpResponse::NotFound().finish());
            };
            alias
        }
    };

    let (target_format, variant, variant_args) =
        prepare_process(&state.config, operations, ext.as_str())?;

    let Some(hash) = state.repo.hash(&source).await? else {
        // Invalid alias
        return Ok(HttpResponse::BadRequest().finish());
    };

    let identifier_opt = state
        .repo
        .variant_identifier(hash.clone(), variant.clone())
        .await?;

    if identifier_opt.is_some() {
        return Ok(HttpResponse::Accepted().finish());
    }

    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    queue_generate(&state.repo, target_format, source, variant, variant_args).await?;

    Ok(HttpResponse::Accepted().finish())
}

/// Fetch file details
#[tracing::instrument(name = "Fetching query details", skip(state))]
async fn details_query<S: Store + 'static>(
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = alias_from_query(query, &state).await?;

    let details = ensure_details(&state, &alias).await?;

    Ok(HttpResponse::Ok().json(&details.into_api_details()))
}

/// Fetch file details
#[tracing::instrument(name = "Fetching details", skip(state))]
async fn details<S: Store + 'static>(
    alias: web::Path<Serde<Alias>>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let details = ensure_details(&state, &alias).await?;

    Ok(HttpResponse::Ok().json(&details.into_api_details()))
}

/// Serve files based on alias query
#[tracing::instrument(name = "Serving file query", skip(state))]
async fn serve_query<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = proxy_alias_from_query(query, &state).await?;

    do_serve(range, alias, state).await
}

/// Serve files
#[tracing::instrument(name = "Serving file", skip(state))]
async fn serve<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: web::Path<Serde<Alias>>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    do_serve(range, Serde::into_inner(alias.into_inner()), state).await
}

async fn do_serve<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: Alias,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let (hash, alias, not_found) = if let Some(hash) = state.repo.hash(&alias).await? {
        (hash, alias, false)
    } else {
        let Some((alias, hash)) = not_found_hash(&state.repo).await? else {
            return Ok(HttpResponse::NotFound().finish());
        };

        (hash, alias, true)
    };

    let Some(identifier) = state.repo.identifier(hash.clone()).await? else {
        tracing::warn!("Original File identifier for hash {hash:?} is missing, queue cleanup task",);
        crate::queue::cleanup_hash(&state.repo, hash).await?;
        return Ok(HttpResponse::NotFound().finish());
    };

    let details = ensure_details(&state, &alias).await?;

    if let Some(public_url) = state.store.public_url(&identifier) {
        return Ok(HttpResponse::SeeOther()
            .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
            .finish());
    }

    ranged_file_resp(&state.store, identifier, range, details, not_found).await
}

#[tracing::instrument(name = "Serving query file headers", skip(state))]
async fn serve_query_head<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = alias_from_query(query, &state).await?;

    do_serve_head(range, alias, state).await
}

#[tracing::instrument(name = "Serving file headers", skip(state))]
async fn serve_head<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: web::Path<Serde<Alias>>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    do_serve_head(range, Serde::into_inner(alias.into_inner()), state).await
}

async fn do_serve_head<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    alias: Alias,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let Some(identifier) = state.repo.identifier_from_alias(&alias).await? else {
        // Invalid alias
        return Ok(HttpResponse::NotFound().finish());
    };

    let details = ensure_details(&state, &alias).await?;

    if let Some(public_url) = state.store.public_url(&identifier) {
        return Ok(HttpResponse::SeeOther()
            .insert_header((actix_web::http::header::LOCATION, public_url.as_str()))
            .finish());
    }

    ranged_file_head_resp(&state.store, identifier, range, details).await
}

async fn ranged_file_head_resp<S: Store + 'static>(
    store: &S,
    identifier: Arc<str>,
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
    identifier: Arc<str>,
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
                        range::chop_store(range, store, &identifier, len).await?,
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
        let stream = crate::stream::from_err(store.to_stream(&identifier, None, None).await?);

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
    let stream = crate::stream::timeout(Duration::from_secs(5), stream);

    let stream = streem::try_from_fn(|yielder| async move {
        let stream = std::pin::pin!(stream);
        let mut streamer = stream.into_streamer();

        while let Some(res) = streamer.next().await {
            tracing::trace!("srv_response: looping");

            let item = res.map_err(Error::from)??;
            yielder.yield_ok(item).await;
        }

        Ok(()) as Result<(), actix_web::Error>
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

#[tracing::instrument(level = "DEBUG", skip(state))]
async fn proxy_alias_from_query<S: Store + 'static>(
    alias_query: AliasQuery,
    state: &State<S>,
) -> Result<Alias, Error> {
    match alias_query {
        AliasQuery::Alias { alias } => Ok(Serde::into_inner(alias)),
        AliasQuery::Proxy { proxy } => {
            let alias = if let Some(alias) = state.repo.related(proxy.clone()).await? {
                alias
            } else if !state.config.server.read_only {
                let stream = download_stream(proxy.as_str(), state).await?;

                // some time has passed, see if we've proxied elsewhere
                if let Some(alias) = state.repo.related(proxy.clone()).await? {
                    alias
                } else {
                    let (alias, token, _) =
                        ingest_inline(stream, state, &Default::default()).await?;

                    // last check, do we succeed or fail to relate the proxy alias
                    if let Err(ProxyAlreadyExists) =
                        state.repo.relate_url(proxy.clone(), alias.clone()).await?
                    {
                        queue::cleanup_alias(&state.repo, alias, token).await?;

                        state
                            .repo
                            .related(proxy)
                            .await?
                            .ok_or(UploadError::MissingAlias)?
                    } else {
                        alias
                    }
                }
            } else {
                return Err(UploadError::ReadOnly.into());
            };

            if !state.config.server.read_only {
                state.repo.accessed_alias(alias.clone()).await?;
            }

            Ok(alias)
        }
    }
}

async fn blurhash<S: Store + 'static>(
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = proxy_alias_from_query(query, &state).await?;

    let hash = state
        .repo
        .hash(&alias)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    let blurhash = if let Some(blurhash) = state.repo.blurhash(hash.clone()).await? {
        blurhash
    } else {
        let details = ensure_details(&state, &alias).await?;
        let blurhash = blurhash::generate(&state, &alias, &details).await?;
        let blurhash: Arc<str> = Arc::from(blurhash);
        state.repo.relate_blurhash(hash, blurhash.clone()).await?;

        blurhash
    };

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "msg": "ok",
        "blurhash": blurhash.as_ref(),
    })))
}

#[derive(serde::Serialize)]
struct PruneResponse {
    complete: bool,
    progress: u64,
    total: u64,
}

#[derive(Debug, serde::Deserialize)]
struct PruneQuery {
    force: Serde<bool>,
}

#[tracing::instrument(name = "Prune missing identifiers", skip(state))]
async fn prune_missing<S>(
    state: web::Data<State<S>>,
    query: Option<web::Query<PruneQuery>>,
) -> Result<HttpResponse, Error> {
    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let total = state.repo.size().await?;

    let progress = if let Some(progress) = state.repo.get("prune-missing-queued").await? {
        progress
            .as_ref()
            .try_into()
            .map(u64::from_be_bytes)
            .unwrap_or(0)
    } else {
        0
    };

    let complete = state.repo.get("prune-missing-complete").await?.is_some();

    let started = state.repo.get("prune-missing-started").await?.is_some();

    if !started || query.is_some_and(|q| *q.force) {
        queue::prune_missing(&state.repo).await?;
    }

    Ok(HttpResponse::Ok().json(PruneResponse {
        complete,
        progress,
        total,
    }))
}

#[tracing::instrument(name = "Spawning variant cleanup", skip(state))]
async fn clean_variants<S>(state: web::Data<State<S>>) -> Result<HttpResponse, Error> {
    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    queue::cleanup_all_variants(&state.repo).await?;
    Ok(HttpResponse::NoContent().finish())
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum AliasQuery {
    Proxy { proxy: url::Url },
    Alias { alias: Serde<Alias> },
}

impl From<ProcessSource> for AliasQuery {
    fn from(value: ProcessSource) -> Self {
        match value {
            ProcessSource::Alias { alias } | ProcessSource::Source { src: alias } => {
                Self::Alias { alias }
            }
            ProcessSource::Proxy { proxy } => Self::Proxy { proxy },
        }
    }
}

#[tracing::instrument(name = "Setting 404 Image", skip(state))]
async fn set_not_found<S>(
    json: web::Json<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let alias = match json.into_inner() {
        AliasQuery::Alias { alias } => Serde::into_inner(alias),
        AliasQuery::Proxy { .. } => {
            return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "msg": "Cannot use proxied media as Not Found image",
                "code": "proxy-not-allowed",
            })));
        }
    };

    state
        .repo
        .hash(&alias)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    state
        .repo
        .set(NOT_FOUND_KEY, alias.to_bytes().into())
        .await?;

    Ok(HttpResponse::Created().json(serde_json::json!({
        "msg": "ok",
    })))
}

async fn alias_from_query<S>(alias_query: AliasQuery, state: &State<S>) -> Result<Alias, Error> {
    match alias_query {
        AliasQuery::Alias { alias } => Ok(Serde::into_inner(alias)),
        AliasQuery::Proxy { proxy } => {
            let alias = state
                .repo
                .related(proxy)
                .await?
                .ok_or(UploadError::MissingProxy)?;

            if !state.config.server.read_only {
                state.repo.accessed_alias(alias.clone()).await?;
            }

            Ok(alias)
        }
    }
}

#[tracing::instrument(name = "Purging file", skip(state))]
async fn purge<S>(
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let alias = alias_from_query(query, &state).await?;

    let aliases = state.repo.aliases_from_alias(&alias).await?;

    let hash = state
        .repo
        .hash(&alias)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    queue::cleanup_hash(&state.repo, hash).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>()
    })))
}

#[tracing::instrument(name = "Deleting alias", skip(state))]
async fn delete_alias<S>(
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    if state.config.server.read_only {
        return Err(UploadError::ReadOnly.into());
    }

    let alias = alias_from_query(query, &state).await?;

    if let Some(token) = state.repo.delete_token(&alias).await? {
        queue::cleanup_alias(&state.repo, alias, token).await?;
    } else {
        return Ok(HttpResponse::NotFound().finish());
    }

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
    })))
}

#[tracing::instrument(name = "Fetching aliases", skip(state))]
async fn aliases<S>(
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = alias_from_query(query, &state).await?;

    let aliases = state.repo.aliases_from_alias(&alias).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>()
    })))
}

#[tracing::instrument(name = "Fetching identifier", skip(state))]
async fn identifier<S>(
    web::Query(query): web::Query<AliasQuery>,
    state: web::Data<State<S>>,
) -> Result<HttpResponse, Error> {
    let alias = alias_from_query(query, &state).await?;

    let identifier = state
        .repo
        .identifier_from_alias(&alias)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "identifier": identifier.as_ref(),
    })))
}

#[tracing::instrument(skip(state))]
async fn healthz<S: Store>(state: web::Data<State<S>>) -> Result<HttpResponse, Error> {
    state.repo.health_check().await?;
    state.store.health_check().await?;
    Ok(HttpResponse::Ok().finish())
}

fn transform_error(error: actix_form_data::Error) -> actix_web::Error {
    let error: Error = error.into();
    let error: actix_web::Error = error.into();
    error
}

fn build_client() -> Result<ClientWithMiddleware, Error> {
    let client = reqwest::Client::builder()
        .user_agent("pict-rs v0.5.0-main")
        .use_rustls_tls()
        .build()
        .map_err(UploadError::BuildClient)?;

    Ok(ClientBuilder::new(client)
        .with(TracingMiddleware::default())
        .build())
}

fn query_config() -> web::QueryConfig {
    web::QueryConfig::default()
        .error_handler(|err, _| Error::from(UploadError::InvalidQuery(err)).into())
}

fn json_config() -> web::JsonConfig {
    web::JsonConfig::default()
        .error_handler(|err, _| Error::from(UploadError::InvalidJson(err)).into())
}

fn configure_endpoints<S: Store + 'static, F: Fn(&mut web::ServiceConfig)>(
    config: &mut web::ServiceConfig,
    state: State<S>,
    extra_config: F,
) {
    config
        .app_data(query_config())
        .app_data(json_config())
        .app_data(web::Data::new(state.clone()))
        .route("/healthz", web::get().to(healthz::<S>))
        .service(
            web::scope("/image")
                .service(
                    web::resource("")
                        .guard(guard::Post())
                        .route(web::post().to(upload::<S>)),
                )
                .service(
                    web::scope("/backgrounded")
                        .service(
                            web::resource("")
                                .guard(guard::Post())
                                .route(web::post().to(upload_backgrounded::<S>)),
                        )
                        .service(web::resource("/claim").route(web::get().to(claim_upload::<S>))),
                )
                .service(web::resource("/download").route(web::get().to(download::<S>)))
                .service(
                    web::resource("/delete/{delete_token}/{filename}")
                        .route(web::delete().to(delete::<S>))
                        .route(web::get().to(delete::<S>)),
                )
                .service(
                    web::scope("/original")
                        .service(
                            web::resource("")
                                .route(web::get().to(serve_query::<S>))
                                .route(web::head().to(serve_query_head::<S>)),
                        )
                        .service(
                            web::resource("/{filename}")
                                .route(web::get().to(serve::<S>))
                                .route(web::head().to(serve_head::<S>)),
                        ),
                )
                .service(web::resource("/blurhash").route(web::get().to(blurhash::<S>)))
                .service(
                    web::resource("/process.{ext}")
                        .route(web::get().to(process::<S>))
                        .route(web::head().to(process_head::<S>)),
                )
                .service(
                    web::resource("/process_backgrounded.{ext}")
                        .route(web::get().to(process_backgrounded::<S>)),
                )
                .service(
                    web::scope("/details")
                        .service(
                            web::scope("/original")
                                .service(web::resource("").route(web::get().to(details_query::<S>)))
                                .service(
                                    web::resource("/{filename}").route(web::get().to(details::<S>)),
                                ),
                        )
                        .service(
                            web::resource("/process.{ext}")
                                .route(web::get().to(process_details::<S>)),
                        ),
                ),
        )
        .service(
            web::scope("/internal")
                .wrap(Internal(state.config.server.api_key.clone()))
                .service(web::resource("/import").route(web::post().to(import::<S>)))
                .service(web::resource("/variants").route(web::delete().to(clean_variants::<S>)))
                .service(web::resource("/purge").route(web::post().to(purge::<S>)))
                .service(web::resource("/delete").route(web::post().to(delete_alias::<S>)))
                .service(web::resource("/aliases").route(web::get().to(aliases::<S>)))
                .service(web::resource("/identifier").route(web::get().to(identifier::<S>)))
                .service(web::resource("/set_not_found").route(web::post().to(set_not_found::<S>)))
                .service(web::resource("/hashes").route(web::get().to(page::<S>)))
                .service(web::resource("/prune_missing").route(web::post().to(prune_missing::<S>)))
                .configure(extra_config),
        );
}

fn spawn_cleanup<S>(state: State<S>) {
    if state.config.server.read_only {
        return;
    }

    crate::sync::spawn("queue-cleanup", async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));

        loop {
            tracing::trace!("queue_cleanup: looping");

            interval.tick().await;

            if let Err(e) = queue::cleanup_outdated_variants(&state.repo).await {
                tracing::warn!(
                    "Failed to spawn cleanup for outdated variants:{}",
                    format!("\n{e}\n{e:?}")
                );
            }

            if let Err(e) = queue::cleanup_outdated_proxies(&state.repo).await {
                tracing::warn!(
                    "Failed to spawn cleanup for outdated proxies:{}",
                    format!("\n{e}\n{e:?}")
                );
            }
        }
    });
}

fn spawn_workers<S>(state: State<S>)
where
    S: Store + 'static,
{
    crate::sync::spawn("cleanup-worker", queue::process_cleanup(state.clone()));
    crate::sync::spawn("process-worker", queue::process_images(state));
}

fn watch_keys(tls: Tls, sender: ChannelSender) -> DropHandle<()> {
    crate::sync::abort_on_drop(crate::sync::spawn("cert-reader", async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.tick().await;

        loop {
            interval.tick().await;

            match tls.open_keys().await {
                Ok(certified_key) => sender.update(certified_key),
                Err(e) => tracing::error!("Failed to open keys {}", format!("{e}")),
            }
        }
    }))
}

async fn launch<
    S: Store + Send + 'static,
    F: Fn(&mut web::ServiceConfig) + Send + Clone + 'static,
>(
    state: State<S>,
    extra_config: F,
) -> color_eyre::Result<()> {
    let address = state.config.server.address;

    let tls = Tls::from_config(&state.config);

    spawn_cleanup(state.clone());

    let server = HttpServer::new(move || {
        let extra_config = extra_config.clone();
        let state = state.clone();

        spawn_workers(state.clone());

        App::new()
            .wrap(Log::new(state.config.tracing.logging.log_requests))
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .wrap(Metrics)
            .wrap(Payload::new())
            .configure(move |sc| configure_endpoints(sc, state.clone(), extra_config))
    });

    if let Some(tls) = tls {
        let certified_key = tls.open_keys().await?;

        let (tx, rx) = rustls_channel_resolver::channel::<32>(certified_key);

        let handle = watch_keys(tls, tx);

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(rx);

        tracing::info!("Starting pict-rs with TLS on {address}");

        server.bind_rustls_0_23(address, config)?.run().await?;

        handle.abort();
        let _ = handle.await;
    } else {
        tracing::info!("Starting pict-rs on {address}");

        server.bind(address)?.run().await?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn migrate_inner<S1>(
    config: Configuration,
    tmp_dir: ArcTmpDir,
    policy_dir: ArcPolicyDir,
    repo: ArcRepo,
    client: ClientWithMiddleware,
    from: S1,
    to: config::primitives::Store,
    skip_missing_files: bool,
    concurrency: usize,
) -> color_eyre::Result<()>
where
    S1: Store + 'static,
{
    match to {
        config::primitives::Store::Filesystem(config::Filesystem { path }) => {
            let store = FileStore::build(path.clone()).await?;

            let to = State {
                config,
                tmp_dir,
                policy_dir,
                repo,
                store,
                client,
            };

            migrate_store(from, to, skip_missing_files, concurrency).await?
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
            let store = ObjectStore::build(
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
            )
            .await?
            .build(client.clone());

            let to = State {
                config,
                tmp_dir,
                policy_dir,
                repo,
                store,
                client,
            };

            migrate_store(from, to, skip_missing_files, concurrency).await?
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
    ///     let configuration = pict_rs::ConfigSource::memory(serde_json::json!({
    ///         "server": {
    ///             "address": "127.0.0.1:8080",
    ///             "temporary_directory": "/tmp/t1"
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

fn sled_extra_config(sc: &mut web::ServiceConfig, repo: SledRepo) {
    sc.app_data(web::Data::new(repo))
        .service(web::resource("/export").route(web::post().to(export_handler)));
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

    /// Install the configured pict-rs metrics collector
    ///
    /// This is a no-op if pict-rs is not configured to export metrics. Applications that register
    /// their own metrics collectors shouldn't call this method.
    pub fn install_metrics(self) -> color_eyre::Result<Self> {
        if let Some(addr) = self.config.metrics.prometheus_address {
            PrometheusBuilder::new()
                .with_http_listener(addr)
                .install()?;
            tracing::info!("Starting prometheus endpoint on {addr}");
        }

        Ok(self)
    }

    /// Install aws-lc-rs as the default crypto provider
    ///
    /// This would happen automatically anyway unless rustls crate features get mixed up
    pub fn install_crypto_provider(self) -> Self {
        if rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .is_err()
        {
            tracing::info!("rustls crypto provider already installed");
        }
        self
    }

    /// Run the pict-rs application on a tokio `LocalSet`
    ///
    /// This must be called from within `tokio::main` directly
    ///
    /// Example:
    /// ```rust
    /// #[tokio::main]
    /// async fn main() -> color_eyre::Result<()> {
    ///     let pict_rs_server = pict_rs::ConfigSource::memory(serde_json::json!({
    ///         "server": {
    ///             "temporary_directory": "/tmp/t2"
    ///         },
    ///         "repo": {
    ///             "type": "sled",
    ///             "path": "/tmp/pict-rs-run-on-localset/sled-repo",
    ///         },
    ///         "store": {
    ///             "type": "filesystem",
    ///             "path": "/tmp/pict-rs-run-on-localset/files",
    ///         },
    ///     }))
    ///         .init::<&str>(None)?
    ///         .run_on_localset();
    ///
    ///     let _ = tokio::time::timeout(std::time::Duration::from_secs(1), pict_rs_server).await;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn run_on_localset(self) -> color_eyre::Result<()> {
        tokio::task::LocalSet::new().run_until(self.run()).await
    }

    /// Run the pict-rs application
    ///
    /// This must be called from within a tokio `LocalSet`, which is created by default for
    /// actix-rt runtimes
    ///
    /// Example:
    /// ```rust
    /// fn main() -> color_eyre::Result<()> {
    ///     actix_web::rt::System::new().block_on(async move {
    ///         let pict_rs_server = pict_rs::ConfigSource::memory(serde_json::json!({
    ///             "server": {
    ///                 "temporary_directory": "/tmp/t3"
    ///             },
    ///             "repo": {
    ///                 "type": "sled",
    ///                 "path": "/tmp/pict-rs-run/sled-repo",
    ///             },
    ///             "store": {
    ///                 "type": "filesystem",
    ///                 "path": "/tmp/pict-rs-run/files",
    ///             },
    ///         }))
    ///         .init::<&str>(None)?
    ///         .run();
    ///
    ///         let _ = tokio::time::timeout(std::time::Duration::from_secs(1), pict_rs_server).await;
    ///
    ///         Ok(())
    ///     })
    /// }
    /// ```
    pub async fn run(self) -> color_eyre::Result<()> {
        #[cfg(feature = "random-errors")]
        tracing::error!("pict-rs has been compiled with with the 'random-errors' feature enabled.");
        #[cfg(feature = "random-errors")]
        tracing::error!("This is not suitable for production environments");

        let PictRsConfiguration { config, operation } = self;

        // describe all the metrics pict-rs produces
        init_metrics::init_metrics();

        let tmp_dir = TmpDir::init(
            &config.server.temporary_directory,
            config.server.cleanup_temporary_directory,
        )
        .await?;
        let policy_dir = magick::write_magick_policy(&config.media, &tmp_dir).await?;

        let client = build_client()?;

        match operation {
            Operation::Run => (),
            Operation::MigrateStore {
                skip_missing_files,
                concurrency,
                from,
                to,
            } => {
                let repo = Repo::open(config.repo.clone()).await?.to_arc();

                match from {
                    config::primitives::Store::Filesystem(config::Filesystem { path }) => {
                        let from = FileStore::build(path.clone()).await?;
                        migrate_inner(
                            config,
                            tmp_dir,
                            policy_dir,
                            repo,
                            client,
                            from,
                            to,
                            skip_missing_files,
                            concurrency,
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
                        )
                        .await?
                        .build(client.clone());

                        migrate_inner(
                            config,
                            tmp_dir,
                            policy_dir,
                            repo,
                            client,
                            from,
                            to,
                            skip_missing_files,
                            concurrency,
                        )
                        .await?;
                    }
                }

                return Ok(());
            }
            Operation::MigrateRepo { from, to } => {
                let from = Repo::open(from).await?.to_arc();
                let to = Repo::open(to).await?.to_arc();

                repo::migrate_repo(from, to).await?;
                return Ok(());
            }
        }

        let repo = Repo::open(config.repo.clone()).await?;

        if config.server.read_only {
            tracing::warn!("Launching in READ ONLY mode");
        }

        match config.store.clone() {
            config::Store::Filesystem(config::Filesystem { path }) => {
                let arc_repo = repo.to_arc();

                let store = FileStore::build(path).await?;

                let state = State {
                    tmp_dir: tmp_dir.clone(),
                    policy_dir: policy_dir.clone(),
                    repo: arc_repo.clone(),
                    store: store.clone(),
                    config: config.clone(),
                    client: client.clone(),
                };

                if arc_repo.get("migrate-0.4").await?.is_none() {
                    if let Some(path) = config.old_repo_path() {
                        if let Some(old_repo) = repo_04::open(path)? {
                            repo::migrate_04(old_repo, state.clone()).await?;
                            arc_repo
                                .set("migrate-0.4", Arc::from(b"migrated".to_vec()))
                                .await?;
                        }
                    }
                }

                match repo {
                    Repo::Sled(sled_repo) => {
                        launch(state, move |sc| sled_extra_config(sc, sled_repo.clone())).await?;
                    }
                    Repo::Postgres(_) => {
                        launch(state, |_| {}).await?;
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
                let arc_repo = repo.to_arc();

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
                )
                .await?
                .build(client.clone());

                let state = State {
                    tmp_dir: tmp_dir.clone(),
                    policy_dir: policy_dir.clone(),
                    repo: arc_repo.clone(),
                    store: store.clone(),
                    config: config.clone(),
                    client: client.clone(),
                };

                if arc_repo.get("migrate-0.4").await?.is_none() {
                    if let Some(path) = config.old_repo_path() {
                        if let Some(old_repo) = repo_04::open(path)? {
                            repo::migrate_04(old_repo, state.clone()).await?;
                            arc_repo
                                .set("migrate-0.4", Arc::from(b"migrated".to_vec()))
                                .await?;
                        }
                    }
                }

                match repo {
                    Repo::Sled(sled_repo) => {
                        launch(state, move |sc| sled_extra_config(sc, sled_repo.clone())).await?;
                    }
                    Repo::Postgres(_) => {
                        launch(state, |_| {}).await?;
                    }
                }
            }
        }

        policy_dir.cleanup().await?;
        tmp_dir.cleanup().await?;

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
