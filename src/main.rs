use actix_form_data::{Field, Form, Value};
use actix_web::{
    guard,
    http::header::{CacheControl, CacheDirective, LastModified, ACCEPT_RANGES},
    middleware::Logger,
    web, App, HttpResponse, HttpResponseBuilder, HttpServer,
};
use awc::Client;
use futures::stream::{Stream, TryStreamExt};
use once_cell::sync::Lazy;
use std::{collections::HashSet, path::PathBuf, pin::Pin, time::SystemTime};
use structopt::StructOpt;
use tracing::{debug, error, info, instrument, Span};
use tracing_subscriber::EnvFilter;

mod config;
mod error;
mod exiv2;
mod ffmpeg;
mod magick;
mod middleware;
mod migrate;
mod processor;
mod range;
mod upload_manager;
mod validate;

use self::{
    config::{Config, Format},
    error::UploadError,
    middleware::{Internal, Tracing},
    upload_manager::{Details, UploadManager},
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

// try moving a file
#[instrument]
async fn safe_move_file(from: PathBuf, to: PathBuf) -> Result<(), UploadError> {
    if let Some(path) = to.parent() {
        debug!("Creating directory {:?}", path);
        actix_fs::create_dir_all(path.to_owned()).await?;
    }

    debug!("Checking if {:?} already exists", to);
    if let Err(e) = actix_fs::metadata(to.clone()).await {
        if e.kind() != Some(std::io::ErrorKind::NotFound) {
            return Err(e.into());
        }
    } else {
        return Err(UploadError::FileExists);
    }

    debug!("Moving {:?} to {:?}", from, to);
    actix_fs::copy(from.clone(), to).await?;
    actix_fs::remove_file(from).await?;
    Ok(())
}

async fn safe_create_parent<P>(path: P) -> Result<(), UploadError>
where
    P: AsRef<std::path::Path>,
{
    if let Some(path) = path.as_ref().parent() {
        debug!("Creating directory {:?}", path);
        actix_fs::create_dir_all(path.to_owned()).await?;
    }

    Ok(())
}

// Try writing to a file
#[instrument(skip(bytes))]
async fn safe_save_file(path: PathBuf, bytes: web::Bytes) -> Result<(), UploadError> {
    if let Some(path) = path.parent() {
        // create the directory for the file
        debug!("Creating directory {:?}", path);
        actix_fs::create_dir_all(path.to_owned()).await?;
    }

    // Only write the file if it doesn't already exist
    debug!("Checking if {:?} already exists", path);
    if let Err(e) = actix_fs::metadata(path.clone()).await {
        if e.kind() != Some(std::io::ErrorKind::NotFound) {
            return Err(e.into());
        }
    } else {
        return Ok(());
    }

    // Open the file for writing
    debug!("Creating {:?}", path);
    let file = actix_fs::file::create(path.clone()).await?;

    // try writing
    debug!("Writing to {:?}", path);
    if let Err(e) = actix_fs::file::write(file, bytes).await {
        error!("Error writing {:?}, {}", path, e);
        // remove file if writing failed before completion
        actix_fs::remove_file(path).await?;
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

fn to_ext(mime: mime::Mime) -> Result<&'static str, UploadError> {
    if mime == mime::IMAGE_PNG {
        Ok(".png")
    } else if mime == mime::IMAGE_JPEG {
        Ok(".jpg")
    } else if mime == video_mp4() {
        Ok(".mp4")
    } else if mime == image_webp() {
        Ok(".webp")
    } else {
        Err(UploadError::UnsupportedFormat)
    }
}

/// Handle responding to succesful uploads
#[instrument(skip(value, manager))]
async fn upload(
    value: Value,
    manager: web::Data<UploadManager>,
) -> Result<HttpResponse, UploadError> {
    let images = value
        .map()
        .and_then(|mut m| m.remove("images"))
        .and_then(|images| images.array())
        .ok_or(UploadError::NoFiles)?;

    let mut files = Vec::new();
    for image in images.into_iter().filter_map(|i| i.file()) {
        if let Some(alias) = image
            .saved_as
            .as_ref()
            .and_then(|s| s.file_name())
            .and_then(|s| s.to_str())
        {
            info!("Uploaded {} as {:?}", image.filename, alias);
            let delete_token = manager.delete_token(alias.to_owned()).await?;

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

            files.push(serde_json::json!({
                "file": alias,
                "delete_token": delete_token,
                "details": details,
            }));
        }
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
#[instrument(skip(client, manager))]
async fn download(
    client: web::Data<Client>,
    manager: web::Data<UploadManager>,
    query: web::Query<UrlQuery>,
) -> Result<HttpResponse, UploadError> {
    let mut res = client.get(&query.url).send().await?;

    if !res.status().is_success() {
        return Err(UploadError::Download(res.status()));
    }

    let fut = res.body().limit(CONFIG.max_file_size() * MEGABYTES);

    let stream = Box::pin(futures::stream::once(fut));

    let alias = manager.upload(stream).await?;
    let delete_token = manager.delete_token(alias.clone()).await?;

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
#[instrument(skip(manager))]
async fn delete(
    manager: web::Data<UploadManager>,
    path_entries: web::Path<(String, String)>,
) -> Result<HttpResponse, UploadError> {
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
) -> Result<(Format, String, PathBuf, Vec<String>), UploadError> {
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

    if alias == "" {
        return Err(UploadError::MissingFilename);
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

async fn process_details(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager>,
    whitelist: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, UploadError> {
    let (_, name, thumbnail_path, _) =
        prepare_process(query, ext.as_str(), &manager, &whitelist).await?;

    let details = manager.variant_details(thumbnail_path, name).await?;

    let details = details.ok_or(UploadError::NoFiles)?;

    Ok(HttpResponse::Ok().json(&details))
}

/// Process files
#[instrument(skip(manager, whitelist))]
async fn process(
    range: Option<range::RangeHeader>,
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager>,
    whitelist: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, UploadError> {
    let (format, name, thumbnail_path, thumbnail_args) =
        prepare_process(query, ext.as_str(), &manager, &whitelist).await?;

    // If the thumbnail doesn't exist, we need to create it
    let thumbnail_exists = if let Err(e) = actix_fs::metadata(thumbnail_path.clone()).await {
        if e.kind() != Some(std::io::ErrorKind::NotFound) {
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

        // Create and save a JPG for motion images (gif, mp4)
        if let Some((updated_path, exists)) =
            self::processor::prepare_image(original_path.clone()).await?
        {
            original_path = updated_path.clone();

            if exists.is_new() {
                // Save the transcoded file in another task
                debug!("Spawning storage task");
                let span = Span::current();
                let manager2 = manager.clone();
                let name = name.clone();
                actix_rt::spawn(async move {
                    let entered = span.enter();
                    if let Err(e) = manager2.store_variant(updated_path, name).await {
                        error!("Error storing variant, {}", e);
                        return;
                    }
                    drop(entered);
                });
            }
        }

        safe_create_parent(&thumbnail_path).await?;

        // apply chain to the provided image
        let dest_file = tmp_file();
        let orig_file = tmp_file();
        actix_fs::copy(original_path, orig_file.clone()).await?;
        magick::process_image(&orig_file, &dest_file, thumbnail_args, format).await?;
        actix_fs::remove_file(orig_file).await?;
        safe_move_file(dest_file, thumbnail_path.clone()).await?;

        let details = if let Some(details) = details {
            details
        } else {
            let details = Details::from_path(&thumbnail_path).await?;
            manager
                .store_variant_details(thumbnail_path.clone(), name.clone(), &details)
                .await?;
            manager
                .store_variant(thumbnail_path.clone(), name.clone())
                .await?;
            details
        };

        return ranged_file_resp(thumbnail_path, range, details).await;
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
async fn details(
    alias: web::Path<String>,
    manager: web::Data<UploadManager>,
) -> Result<HttpResponse, UploadError> {
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
#[instrument(skip(manager))]
async fn serve(
    range: Option<range::RangeHeader>,
    alias: web::Path<String>,
    manager: web::Data<UploadManager>,
) -> Result<HttpResponse, UploadError> {
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
) -> Result<HttpResponse, UploadError> {
    let (builder, stream) = match range {
        //Range header exists - return as ranged
        Some(range_header) => {
            if !range_header.is_bytes() {
                return Err(UploadError::Range);
            }

            if range_header.is_empty() {
                return Err(UploadError::Range);
            } else if range_header.len() == 1 {
                let file = actix_fs::file::open(path).await?;

                let (file, meta) = actix_fs::file::metadata(file).await?;

                let range = range_header.ranges().next().unwrap();

                let mut builder = HttpResponse::PartialContent();
                builder.insert_header(range.to_content_range(meta.len()));

                (builder, range.chop_file(file).await?)
            } else {
                return Err(UploadError::Range);
            }
        }
        //No Range header in the request - return the entire document
        None => {
            let stream = actix_fs::read_to_stream(path)
                .await?
                .faster()
                .map_err(UploadError::from);
            let stream: Pin<Box<dyn Stream<Item = Result<web::Bytes, UploadError>>>> =
                Box::pin(stream);
            (HttpResponse::Ok(), stream)
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

async fn purge(
    query: web::Query<FileOrAlias>,
    upload_manager: web::Data<UploadManager>,
) -> Result<HttpResponse, UploadError> {
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

async fn aliases(
    query: web::Query<FileOrAlias>,
    upload_manager: web::Data<UploadManager>,
) -> Result<HttpResponse, UploadError> {
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

async fn filename_by_alias(
    query: web::Query<ByAlias>,
    upload_manager: web::Data<UploadManager>,
) -> Result<HttpResponse, UploadError> {
    let filename = upload_manager.from_alias(query.into_inner().alias).await?;

    Ok(HttpResponse::Ok().json(&serde_json::json!({
        "msg": "ok",
        "filename": filename,
    })))
}

#[actix_rt::main]
async fn main() -> Result<(), anyhow::Error> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let manager = UploadManager::new(CONFIG.data_dir(), CONFIG.format()).await?;

    // Create a new Multipart Form validator
    //
    // This form is expecting a single array field, 'images' with at most 10 files in it
    let manager2 = manager.clone();
    let form = Form::new()
        .max_files(10)
        .max_file_size(CONFIG.max_file_size() * MEGABYTES)
        .transform_error(|e| UploadError::from(e).into())
        .field(
            "images",
            Field::array(Field::file(move |filename, _, stream| {
                let manager = manager2.clone();

                async move {
                    let span = tracing::info_span!("file-upload", ?filename);
                    let entered = span.enter();

                    let res = manager.upload(stream).await.map(|alias| {
                        let mut path = PathBuf::new();
                        path.push(alias);
                        Some(path)
                    });
                    drop(entered);
                    res
                }
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
        .transform_error(|e| UploadError::from(e).into())
        .field(
            "images",
            Field::array(Field::file(move |filename, content_type, stream| {
                let manager = manager2.clone();

                async move {
                    let span = tracing::info_span!("file-import", ?filename);
                    let entered = span.enter();

                    let res = manager
                        .import(filename, content_type, validate_imports, stream)
                        .await
                        .map(|alias| {
                            let mut path = PathBuf::new();
                            path.push(alias);
                            Some(path)
                        });
                    drop(entered);
                    res
                }
            })),
        );

    HttpServer::new(move || {
        let client = Client::builder()
            .header("User-Agent", "pict-rs v0.1.0-master")
            .finish();

        App::new()
            .wrap(Logger::default())
            .wrap(Tracing)
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

    if actix_fs::metadata(&*TMP_DIR).await.is_ok() {
        actix_fs::remove_dir_all(&*TMP_DIR).await?;
    }

    Ok(())
}
