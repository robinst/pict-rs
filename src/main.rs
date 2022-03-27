use actix_form_data::{Field, Form, Value};
use actix_web::{
    guard,
    http::header::{CacheControl, CacheDirective, LastModified, Range, ACCEPT_RANGES},
    web, App, HttpResponse, HttpResponseBuilder, HttpServer,
};
use awc::Client;
use futures_util::{
    stream::{empty, once},
    Stream,
};
use once_cell::sync::Lazy;
use std::{
    collections::HashSet,
    future::ready,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
    time::SystemTime,
};
use tokio::{io::AsyncReadExt, sync::Semaphore};
use tracing::{debug, error, info, instrument, Span};
use tracing_actix_web::TracingLogger;
use tracing_awc::Tracing;
use tracing_futures::Instrument;

mod concurrent_processor;
mod config;
mod details;
mod either;
mod error;
mod exiftool;
mod ffmpeg;
mod file;
mod init_tracing;
mod magick;
mod map_error;
mod middleware;
mod migrate;
mod process;
mod processor;
mod range;
mod repo;
mod serde_str;
mod store;
mod tmp_file;
mod upload_manager;
mod validate;

use self::{
    concurrent_processor::CancelSafeProcessor,
    config::{CommandConfig, Config, Format, RequiredFilesystemStorage, RequiredObjectStorage},
    details::Details,
    either::Either,
    error::{Error, UploadError},
    init_tracing::init_tracing,
    magick::details_hint,
    middleware::{Deadline, Internal},
    migrate::LatestDb,
    repo::{Alias, DeleteToken, Repo},
    store::{file_store::FileStore, object_store::ObjectStore, Store},
    upload_manager::{UploadManager, UploadManagerSession},
};

const MEGABYTES: usize = 1024 * 1024;
const MINUTES: u32 = 60;
const HOURS: u32 = 60 * MINUTES;
const DAYS: u32 = 24 * HOURS;

static CONFIG: Lazy<Config> = Lazy::new(|| Config::build().unwrap());
static PROCESS_SEMAPHORE: Lazy<Semaphore> =
    Lazy::new(|| Semaphore::new(num_cpus::get().saturating_sub(1).max(1)));

/// Handle responding to succesful uploads
#[instrument(name = "Uploaded files", skip(value, manager))]
async fn upload<S: Store>(
    value: Value<UploadManagerSession<S>>,
    manager: web::Data<UploadManager>,
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

            let identifier = manager.identifier_from_alias::<S>(alias).await?;
            let details = manager.details(&identifier).await?;

            let details = if let Some(details) = details {
                debug!("details exist");
                details
            } else {
                debug!("generating new details from {:?}", identifier);
                let hint = details_hint(alias);
                let new_details =
                    Details::from_store((**store).clone(), identifier.clone(), hint).await?;
                debug!("storing details for {:?}", identifier);
                manager.store_details(&identifier, &new_details).await?;
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
    manager: web::Data<UploadManager>,
    store: web::Data<S>,
    query: web::Query<UrlQuery>,
) -> Result<HttpResponse, Error> {
    let res = client.get(&query.url).send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()).into());
    }

    let stream = Limit::new(
        map_error::map_crate_error(res),
        (CONFIG.max_file_size() * MEGABYTES) as u64,
    );

    futures_util::pin_mut!(stream);

    let permit = PROCESS_SEMAPHORE.acquire().await?;
    let session = manager.session((**store).clone()).upload(stream).await?;
    let alias = session.alias().unwrap().to_owned();
    drop(permit);
    let delete_token = session.delete_token().await?;

    let identifier = manager.identifier_from_alias::<S>(&alias).await?;

    let details = manager.details(&identifier).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&alias);
        let new_details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
        manager.store_details(&identifier, &new_details).await?;
        new_details
    };

    session.succeed();
    Ok(HttpResponse::Created().json(&serde_json::json!({
        "msg": "ok",
        "files": [{
            "file": alias.to_string(),
            "delete_token": delete_token.to_string(),
            "details": details,
        }]
    })))
}

/// Delete aliases and files
#[instrument(name = "Deleting file", skip(manager))]
async fn delete<S: Store>(
    manager: web::Data<UploadManager>,
    store: web::Data<S>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, Error> {
    let (token, alias) = path_entries.into_inner();

    let token = DeleteToken::from_existing(&token);
    let alias = Alias::from_existing(&alias);

    manager.delete((**store).clone(), alias, token).await?;

    Ok(HttpResponse::NoContent().finish())
}

type ProcessQuery = Vec<(String, String)>;

fn prepare_process(
    query: web::Query<ProcessQuery>,
    ext: &str,
    filters: &Option<HashSet<String>>,
) -> Result<(Format, Alias, PathBuf, Vec<String>), Error> {
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

    let alias = Alias::from_existing(&alias);

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

    let (thumbnail_path, thumbnail_args) = self::processor::build_chain(&operations)?;

    Ok((format, alias, thumbnail_path, thumbnail_args))
}

#[instrument(name = "Fetching derived details", skip(manager, filters))]
async fn process_details<S: Store>(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager>,
    store: web::Data<S>,
    filters: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, Error> {
    let (_, alias, thumbnail_path, _) = prepare_process(query, ext.as_str(), &filters)?;

    let identifier = manager
        .variant_identifier::<S>(&alias, &thumbnail_path)
        .await?
        .ok_or(UploadError::MissingAlias)?;

    let details = manager.details(&identifier).await?;

    let details = details.ok_or(UploadError::NoFiles)?;

    Ok(HttpResponse::Ok().json(&details))
}

/// Process files
#[instrument(name = "Serving processed image", skip(manager, filters))]
async fn process<S: Store + 'static>(
    range: Option<web::Header<Range>>,
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager>,
    store: web::Data<S>,
    filters: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, Error> {
    let (format, alias, thumbnail_path, thumbnail_args) =
        prepare_process(query, ext.as_str(), &filters)?;

    let identifier_opt = manager
        .variant_identifier::<S>(&alias, &thumbnail_path)
        .await?;

    if let Some(identifier) = identifier_opt {
        let details_opt = manager.details(&identifier).await?;

        let details = if let Some(details) = details_opt {
            details
        } else {
            let hint = details_hint(&alias);
            let details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
            manager.store_details(&identifier, &details).await?;
            details
        };

        return ranged_file_resp(&**store, identifier, range, details).await;
    }

    let identifier = manager
        .still_identifier_from_alias((**store).clone(), &alias)
        .await?;

    let thumbnail_path2 = thumbnail_path.clone();
    let process_fut = async {
        let thumbnail_path = thumbnail_path2;

        let permit = PROCESS_SEMAPHORE.acquire().await?;

        let mut processed_reader = crate::magick::process_image_store_read(
            (**store).clone(),
            identifier,
            thumbnail_args,
            format,
        )?;

        let mut vec = Vec::new();
        processed_reader.read_to_end(&mut vec).await?;
        let bytes = web::Bytes::from(vec);

        drop(permit);

        let details = Details::from_bytes(bytes.clone(), format.as_hint()).await?;

        let save_span = tracing::info_span!(
            parent: None,
            "Saving variant information",
            path = tracing::field::debug(&thumbnail_path),
            name = tracing::field::display(&alias),
        );
        save_span.follows_from(Span::current());
        let details2 = details.clone();
        let bytes2 = bytes.clone();
        let alias2 = alias.clone();
        actix_rt::spawn(
            async move {
                let identifier = match store.save_bytes(bytes2).await {
                    Ok(identifier) => identifier,
                    Err(e) => {
                        tracing::warn!("Failed to generate directory path: {}", e);
                        return;
                    }
                };
                if let Err(e) = manager.store_details(&identifier, &details2).await {
                    tracing::warn!("Error saving variant details: {}", e);
                    return;
                }
                if let Err(e) = manager
                    .store_variant(&alias2, &thumbnail_path, &identifier)
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

/// Fetch file details
#[instrument(name = "Fetching details", skip(manager))]
async fn details<S: Store>(
    alias: web::Path<String>,
    manager: web::Data<UploadManager>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = alias.into_inner();
    let alias = Alias::from_existing(&alias);

    let identifier = manager.identifier_from_alias::<S>(&alias).await?;

    let details = manager.details(&identifier).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&alias);
        let new_details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
        manager.store_details(&identifier, &new_details).await?;
        new_details
    };

    Ok(HttpResponse::Ok().json(&details))
}

/// Serve files
#[instrument(name = "Serving file", skip(manager))]
async fn serve<S: Store>(
    range: Option<web::Header<Range>>,
    alias: web::Path<String>,
    manager: web::Data<UploadManager>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = alias.into_inner();
    let alias = Alias::from_existing(&alias);
    let identifier = manager.identifier_from_alias::<S>(&alias).await?;

    let details = manager.details(&identifier).await?;

    let details = if let Some(details) = details {
        details
    } else {
        let hint = details_hint(&alias);
        let details = Details::from_store((**store).clone(), identifier.clone(), hint).await?;
        manager.store_details(&identifier, &details).await?;
        details
    };

    ranged_file_resp(&**store, identifier, range, details).await
}

async fn ranged_file_resp<S: Store>(
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
                    Either::left(Either::left(map_error::map_crate_error(
                        range::chop_store(range, store, &identifier, len).await?,
                    ))),
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
        let stream = map_error::map_crate_error(store.to_stream(&identifier, None, None).await?);
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
    alias: String,
}

#[instrument(name = "Purging file", skip(upload_manager))]
async fn purge<S: Store>(
    query: web::Query<AliasQuery>,
    upload_manager: web::Data<UploadManager>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = Alias::from_existing(&query.alias);
    let aliases = upload_manager.aliases_by_alias(&alias).await?;

    for alias in aliases.iter() {
        upload_manager
            .delete_without_token((**store).clone(), alias.to_owned())
            .await?;
    }

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "aliases": aliases.iter().map(|a| a.to_string()).collect::<Vec<_>>()
    })))
}

#[instrument(name = "Fetching aliases", skip(upload_manager))]
async fn aliases<S: Store>(
    query: web::Query<AliasQuery>,
    upload_manager: web::Data<UploadManager>,
    store: web::Data<S>,
) -> Result<HttpResponse, Error> {
    let alias = Alias::from_existing(&query.alias);
    let aliases = upload_manager.aliases_by_alias(&alias).await?;

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

async fn launch<S: Store + Clone + 'static>(
    manager: UploadManager,
    store: S,
) -> anyhow::Result<()> {
    // Create a new Multipart Form validator
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let manager2 = manager.clone();
    let store2 = store.clone();
    let form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.max_file_size() * MEGABYTES)
        .transform_error(transform_error)
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let store = store2.clone();
                let manager = manager2.clone();

                let span = tracing::info_span!("file-upload", ?filename);

                async move {
                    let permit = PROCESS_SEMAPHORE.acquire().await?;

                    let res = manager
                        .session(store)
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
    let store2 = store.clone();
    let import_form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.max_file_size() * MEGABYTES)
        .transform_error(transform_error)
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let store = store2.clone();
                let manager = manager2.clone();

                let span = tracing::info_span!("file-import", ?filename);

                async move {
                    let permit = PROCESS_SEMAPHORE.acquire().await?;

                    let res = manager
                        .session(store)
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
        App::new()
            .wrap(TracingLogger::default())
            .wrap(Deadline)
            .app_data(web::Data::new(store.clone()))
            .app_data(web::Data::new(manager.clone()))
            .app_data(web::Data::new(build_client()))
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
                    .service(web::resource("/aliases").route(web::get().to(aliases::<S>))),
            )
    })
    .bind(CONFIG.bind_address())?
    .run()
    .await?;

    crate::tmp_file::remove_tmp_dir().await?;

    Ok(())
}

async fn migrate_inner<S1>(
    manager: &UploadManager,
    repo: &Repo,
    from: S1,
    to: &config::Storage,
) -> anyhow::Result<()>
where
    S1: Store,
{
    match to {
        config::Storage::Filesystem(RequiredFilesystemStorage { path }) => {
            let to = FileStore::build(path.clone(), repo.clone()).await?;
            manager.migrate_store::<S1, FileStore>(from, to).await?;
        }
        config::Storage::ObjectStorage(RequiredObjectStorage {
            bucket_name,
            region,
            access_key,
            secret_key,
            security_token,
            session_token,
        }) => {
            let to = ObjectStore::build(
                bucket_name,
                region.clone(),
                access_key.clone(),
                secret_key.clone(),
                security_token.clone(),
                session_token.clone(),
                repo.clone(),
                build_reqwest_client()?,
            )
            .await?;

            manager.migrate_store::<S1, ObjectStore>(from, to).await?;
        }
    }

    Ok(())
}

#[actix_rt::main]
async fn main() -> anyhow::Result<()> {
    init_tracing(
        "pict-rs",
        CONFIG.opentelemetry_url(),
        CONFIG.console_buffer_capacity(),
    )?;

    let repo = Repo::open(CONFIG.repo())?;

    let db = LatestDb::exists(CONFIG.data_dir()).migrate()?;
    repo.from_db(db).await?;

    let manager = UploadManager::new(repo.clone(), CONFIG.format()).await?;

    match CONFIG.command()? {
        CommandConfig::Run => (),
        CommandConfig::Dump { path } => {
            let configuration = toml::to_string_pretty(&*CONFIG)?;
            tokio::fs::write(path, configuration).await?;
            return Ok(());
        }
        CommandConfig::MigrateRepo { to: _ } => {
            unimplemented!("Repo migrations are currently unsupported")
        }
        CommandConfig::MigrateStore { to } => {
            let from = CONFIG.store()?;

            match from {
                config::Storage::Filesystem(RequiredFilesystemStorage { path }) => {
                    let from = FileStore::build(path.clone(), repo.clone()).await?;
                    migrate_inner(&manager, &repo, from, &to).await?;
                }
                config::Storage::ObjectStorage(RequiredObjectStorage {
                    bucket_name,
                    region,
                    access_key,
                    secret_key,
                    security_token,
                    session_token,
                }) => {
                    let from = ObjectStore::build(
                        &bucket_name,
                        region,
                        access_key,
                        secret_key,
                        security_token,
                        session_token,
                        repo.clone(),
                        build_reqwest_client()?,
                    )
                    .await?;

                    migrate_inner(&manager, &repo, from, &to).await?;
                }
            }

            return Ok(());
        }
    }

    match CONFIG.store()? {
        config::Storage::Filesystem(RequiredFilesystemStorage { path }) => {
            let store = FileStore::build(path, repo).await?;
            launch(manager, store).await
        }
        config::Storage::ObjectStorage(RequiredObjectStorage {
            bucket_name,
            region,
            access_key,
            secret_key,
            security_token,
            session_token,
        }) => {
            let store = ObjectStore::build(
                &bucket_name,
                region,
                access_key,
                secret_key,
                security_token,
                session_token,
                repo,
                build_reqwest_client()?,
            )
            .await?;

            launch(manager, store).await
        }
    }
}
