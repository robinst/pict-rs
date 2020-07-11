use actix_form_data::{Field, Form, Value};
use actix_web::{
    client::Client,
    guard,
    http::header::{CacheControl, CacheDirective},
    middleware::{Compress, Logger},
    web, App, HttpResponse, HttpServer,
};
use futures::stream::{Stream, TryStreamExt};
use once_cell::sync::Lazy;
use std::{collections::HashSet, path::PathBuf, sync::Once};
use structopt::StructOpt;
use tracing::{debug, error, info, instrument, Span};
use tracing_subscriber::EnvFilter;

mod config;
mod error;
mod middleware;
mod migrate;
mod processor;
mod upload_manager;
mod validate;

use self::{
    config::Config,
    error::UploadError,
    middleware::{Internal, Tracing},
    processor::process_image,
    upload_manager::UploadManager,
    validate::{image_webp, video_mp4},
};

const MEGABYTES: usize = 1024 * 1024;
const HOURS: u32 = 60 * 60;

static CONFIG: Lazy<Config> = Lazy::new(|| Config::from_args());
static MAGICK_INIT: Once = Once::new();

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

// Try writing to a file
#[instrument(skip(bytes))]
async fn safe_save_file(path: PathBuf, bytes: bytes::Bytes) -> Result<(), UploadError> {
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

    let s: String = Alphanumeric.sample_iter(rng).take(limit).collect();

    let name = format!("{}.tmp", s);

    let mut path = std::env::temp_dir();
    path.push("pict-rs");
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

fn from_name(name: &str) -> Result<mime::Mime, UploadError> {
    match name
        .rsplit('.')
        .next()
        .ok_or(UploadError::UnsupportedFormat)?
    {
        "jpg" => Ok(mime::IMAGE_JPEG),
        "webp" => Ok(image_webp()),
        "png" => Ok(mime::IMAGE_PNG),
        "mp4" => Ok(video_mp4()),
        "gif" => Ok(mime::IMAGE_GIF),
        _ => Err(UploadError::UnsupportedFormat),
    }
}

fn from_ext(ext: &str) -> Result<mime::Mime, UploadError> {
    match ext {
        "jpg" => Ok(mime::IMAGE_JPEG),
        "png" => Ok(mime::IMAGE_PNG),
        "webp" => Ok(image_webp()),
        _ => Err(UploadError::UnsupportedFormat),
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
        if let Some(saved_as) = image
            .saved_as
            .as_ref()
            .and_then(|s| s.file_name())
            .and_then(|s| s.to_str())
        {
            info!("Uploaded {} as {:?}", image.filename, saved_as);
            let delete_token = manager.delete_token(saved_as.to_owned()).await?;
            files.push(serde_json::json!({
                "file": saved_as,
                "delete_token": delete_token
            }));
        }
    }

    Ok(HttpResponse::Created().json(serde_json::json!({
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

    Ok(HttpResponse::Created().json(serde_json::json!({
        "msg": "ok",
        "files": [{
            "file": alias,
            "delete_token": delete_token
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

/// Process files
#[instrument(skip(manager, whitelist))]
async fn process(
    query: web::Query<ProcessQuery>,
    ext: web::Path<String>,
    manager: web::Data<UploadManager>,
    whitelist: web::Data<Option<HashSet<String>>>,
) -> Result<HttpResponse, UploadError> {
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

    let ext = ext.into_inner();
    let content_type = from_ext(&ext)?;
    let processed_name = format!("{}.{}", name, ext);
    let base = manager.image_dir();
    let mut path = self::processor::build_path(base, &chain, processed_name);

    if let Some((updated_path, exists)) = self::processor::prepare_image(path.clone()).await? {
        path = updated_path.clone();

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

    // If the thumbnail doesn't exist, we need to create it
    if let Err(e) = actix_fs::metadata(path.clone()).await {
        if e.kind() != Some(std::io::ErrorKind::NotFound) {
            error!("Error looking up processed image, {}", e);
            return Err(e.into());
        }

        let mut original_path = manager.image_dir();
        original_path.push(name.clone());

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

        // apply chain to the provided image
        let img_bytes = process_image(original_path.clone(), chain).await?;

        let path2 = path.clone();
        let img_bytes2 = img_bytes.clone();

        // Save the file in another task, we want to return the thumbnail now
        debug!("Spawning storage task");
        let span = Span::current();
        actix_rt::spawn(async move {
            let entered = span.enter();
            if let Err(e) = manager.store_variant(path2.clone(), name).await {
                error!("Error storing variant, {}", e);
                return;
            }

            if let Err(e) = safe_save_file(path2, img_bytes2).await {
                error!("Error saving file, {}", e);
            }
            drop(entered);
        });

        return Ok(srv_response(
            Box::pin(futures::stream::once(async {
                Ok(img_bytes) as Result<_, UploadError>
            })),
            content_type,
        ));
    }

    let stream = actix_fs::read_to_stream(path).await?;

    Ok(srv_response(stream, content_type))
}

/// Serve files
#[instrument(skip(manager))]
async fn serve(
    alias: web::Path<String>,
    manager: web::Data<UploadManager>,
) -> Result<HttpResponse, UploadError> {
    let name = manager.from_alias(alias.into_inner()).await?;
    let content_type = from_name(&name)?;
    let mut path = manager.image_dir();
    path.push(name);

    let stream = actix_fs::read_to_stream(path).await?;

    Ok(srv_response(stream, content_type))
}

// A helper method to produce responses with proper cache headers
fn srv_response<S, E>(stream: S, ext: mime::Mime) -> HttpResponse
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Unpin + 'static,
    E: Into<UploadError>,
{
    HttpResponse::Ok()
        .set(CacheControl(vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(24 * HOURS),
            CacheDirective::Extension("immutable".to_owned(), None),
        ]))
        .content_type(ext.to_string())
        .streaming(stream.err_into())
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

    Ok(HttpResponse::Ok().json(serde_json::json!({
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

    Ok(HttpResponse::Ok().json(serde_json::json!({
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

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "msg": "ok",
        "filename": filename,
    })))
}

#[actix_rt::main]
async fn main() -> Result<(), anyhow::Error> {
    MAGICK_INIT.call_once(|| {
        magick_rust::magick_wand_genesis();
    });

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
        let client = Client::build()
            .header("User-Agent", "pict-rs v0.1.0-master")
            .finish();

        App::new()
            .wrap(Compress::default())
            .wrap(Logger::default())
            .wrap(Tracing)
            .data(manager.clone())
            .data(client)
            .data(CONFIG.filter_whitelist())
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
                    .service(web::resource("/process.{ext}").route(web::get().to(process))),
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

    Ok(())
}
