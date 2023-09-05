use crate::{
    bytes_stream::BytesStream,
    error_code::ErrorCode,
    repo::ArcRepo,
    store::Store,
    stream::{IntoStreamer, LocalBoxStream, StreamMap},
};
use actix_rt::task::JoinError;
use actix_web::{
    error::BlockingError,
    http::{
        header::{ByteRangeSpec, Range, CONTENT_LENGTH},
        StatusCode,
    },
    web::Bytes,
};
use base64::{prelude::BASE64_STANDARD, Engine};
use futures_core::Stream;
use reqwest::{header::RANGE, Body, Response};
use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use rusty_s3::{actions::S3Action, Bucket, BucketError, Credentials, UrlStyle};
use std::{string::FromUtf8Error, sync::Arc, time::Duration};
use storage_path_generator::{Generator, Path};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tracing::Instrument;
use url::Url;

use super::StoreError;

const CHUNK_SIZE: usize = 8_388_608; // 8 Mebibytes, min is 5 (5_242_880);

// - Settings Tree
//   - last-path -> last generated path

const GENERATOR_KEY: &str = "last-path";

#[derive(Debug, thiserror::Error)]
pub(crate) enum ObjectError {
    #[error("Failed to generate path")]
    PathGenerator(#[from] storage_path_generator::PathError),

    #[error("Failed to generate request")]
    S3(#[from] BucketError),

    #[error("IO Error")]
    IO(#[from] std::io::Error),

    #[error("Error making request")]
    RequestMiddleware(#[from] reqwest_middleware::Error),

    #[error("Error in request response")]
    Request(#[from] reqwest::Error),

    #[error("Failed to parse string")]
    Utf8(#[from] FromUtf8Error),

    #[error("Failed to parse xml")]
    Xml(#[from] quick_xml::de::DeError),

    #[error("Invalid length")]
    Length,

    #[error("Invalid etag response")]
    Etag,

    #[error("Task cancelled")]
    Canceled,

    #[error("Invalid status: {0}\n{1}")]
    Status(StatusCode, String),
}

impl ObjectError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::PathGenerator(_) => ErrorCode::PARSE_PATH_ERROR,
            Self::S3(_)
            | Self::RequestMiddleware(_)
            | Self::Request(_)
            | Self::Xml(_)
            | Self::Length
            | Self::Etag
            | Self::Status(_, _) => ErrorCode::OBJECT_REQUEST_ERROR,
            Self::IO(_) => ErrorCode::OBJECT_IO_ERROR,
            Self::Utf8(_) => ErrorCode::PARSE_OBJECT_ID_ERROR,
            Self::Canceled => ErrorCode::PANIC,
        }
    }
}

impl From<JoinError> for ObjectError {
    fn from(_: JoinError) -> Self {
        Self::Canceled
    }
}

impl From<BlockingError> for ObjectError {
    fn from(_: BlockingError) -> Self {
        Self::Canceled
    }
}

#[derive(Clone)]
pub(crate) struct ObjectStore {
    path_gen: Generator,
    repo: ArcRepo,
    bucket: Bucket,
    credentials: Credentials,
    client: ClientWithMiddleware,
    signature_expiration: Duration,
    client_timeout: Duration,
    public_endpoint: Option<Url>,
}

#[derive(Clone)]
pub(crate) struct ObjectStoreConfig {
    path_gen: Generator,
    repo: ArcRepo,
    bucket: Bucket,
    credentials: Credentials,
    signature_expiration: u64,
    client_timeout: u64,
    public_endpoint: Option<Url>,
}

#[derive(serde::Deserialize, Debug)]
struct InitiateMultipartUploadResponse {
    #[serde(rename = "Bucket")]
    _bucket: String,
    #[serde(rename = "Key")]
    _key: String,
    #[serde(rename = "UploadId")]
    upload_id: String,
}

impl ObjectStoreConfig {
    pub(crate) fn build(self, client: ClientWithMiddleware) -> ObjectStore {
        ObjectStore {
            path_gen: self.path_gen,
            repo: self.repo,
            bucket: self.bucket,
            credentials: self.credentials,
            client,
            signature_expiration: Duration::from_secs(self.signature_expiration),
            client_timeout: Duration::from_secs(self.client_timeout),
            public_endpoint: self.public_endpoint,
        }
    }
}

fn payload_to_io_error(e: reqwest::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

#[tracing::instrument(skip(stream))]
async fn read_chunk<S>(stream: &mut S) -> Result<BytesStream, ObjectError>
where
    S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
{
    let mut buf = BytesStream::new();

    let mut stream = stream.into_streamer();

    while buf.len() < CHUNK_SIZE {
        if let Some(res) = stream.next().await {
            buf.add_bytes(res?)
        } else {
            break;
        }
    }

    Ok(buf)
}

async fn status_error(response: Response) -> StoreError {
    let status = response.status();

    let body = match response.text().await {
        Err(e) => return ObjectError::Request(e).into(),
        Ok(body) => body,
    };

    ObjectError::Status(status, body).into()
}

#[async_trait::async_trait(?Send)]
impl Store for ObjectStore {
    async fn health_check(&self) -> Result<(), StoreError> {
        let response = self
            .head_bucket_request()
            .await?
            .send()
            .await
            .map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        Ok(())
    }

    async fn save_async_read<Reader>(
        &self,
        reader: Reader,
        content_type: mime::Mime,
    ) -> Result<Arc<str>, StoreError>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        self.save_stream(ReaderStream::new(reader), content_type)
            .await
    }

    #[tracing::instrument(skip_all)]
    async fn save_stream<S>(
        &self,
        mut stream: S,
        content_type: mime::Mime,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        let first_chunk = read_chunk(&mut stream).await?;

        if first_chunk.len() < CHUNK_SIZE {
            drop(stream);
            let (req, object_id) = self
                .put_object_request(first_chunk.len(), content_type)
                .await?;
            let response = req
                .body(Body::wrap_stream(first_chunk))
                .send()
                .await
                .map_err(ObjectError::from)?;

            if !response.status().is_success() {
                return Err(status_error(response).await);
            }

            return Ok(object_id);
        }

        let mut first_chunk = Some(first_chunk);

        let (req, object_id) = self.create_multipart_request(content_type).await?;
        let response = req.send().await.map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        let body = response.bytes().await.map_err(ObjectError::Request)?;
        let body: InitiateMultipartUploadResponse =
            quick_xml::de::from_reader(&*body).map_err(ObjectError::from)?;
        let upload_id = &body.upload_id;

        // hack-ish: use async block as Result boundary
        let res = async {
            let mut complete = false;
            let mut part_number = 0;
            let mut futures = Vec::new();

            while !complete {
                part_number += 1;

                let buf = if let Some(buf) = first_chunk.take() {
                    buf
                } else {
                    read_chunk(&mut stream).await?
                };

                complete = buf.len() < CHUNK_SIZE;

                let this = self.clone();

                let object_id2 = object_id.clone();
                let upload_id2 = upload_id.clone();
                let handle = crate::sync::spawn(
                    async move {
                        let response = this
                            .create_upload_part_request(
                                buf.clone(),
                                &object_id2,
                                part_number,
                                &upload_id2,
                            )
                            .await?
                            .body(Body::wrap_stream(buf))
                            .send()
                            .await
                            .map_err(ObjectError::from)?;

                        if !response.status().is_success() {
                            return Err(status_error(response).await);
                        }

                        let etag = response
                            .headers()
                            .get("etag")
                            .ok_or(ObjectError::Etag)?
                            .to_str()
                            .map_err(|_| ObjectError::Etag)?
                            .to_string();

                        // early-drop response to close its tracing spans
                        drop(response);

                        Ok(etag) as Result<String, StoreError>
                    }
                    .instrument(tracing::Span::current()),
                );

                futures.push(handle);
            }

            // early-drop stream to allow the next Part to be polled concurrently
            drop(stream);

            let mut etags = Vec::new();

            for future in futures {
                etags.push(future.await.map_err(ObjectError::from)??);
            }

            let response = self
                .send_complete_multipart_request(
                    &object_id,
                    upload_id,
                    etags.iter().map(|s| s.as_ref()),
                )
                .await
                .map_err(ObjectError::from)?;

            if !response.status().is_success() {
                return Err(status_error(response).await);
            }

            Ok(()) as Result<(), StoreError>
        }
        .await;

        if let Err(e) = res {
            self.create_abort_multipart_request(&object_id, upload_id)
                .send()
                .await
                .map_err(ObjectError::from)?;
            return Err(e);
        }

        Ok(object_id)
    }

    #[tracing::instrument(skip_all)]
    async fn save_bytes(
        &self,
        bytes: Bytes,
        content_type: mime::Mime,
    ) -> Result<Arc<str>, StoreError> {
        let (req, object_id) = self.put_object_request(bytes.len(), content_type).await?;

        let response = req.body(bytes).send().await.map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        Ok(object_id)
    }

    fn public_url(&self, identifier: &Arc<str>) -> Option<url::Url> {
        self.public_endpoint.clone().map(|mut endpoint| {
            endpoint.set_path(identifier.as_ref());
            endpoint
        })
    }

    #[tracing::instrument(skip(self))]
    async fn to_stream(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<LocalBoxStream<'static, std::io::Result<Bytes>>, StoreError> {
        let response = self
            .get_object_request(identifier, from_start, len)
            .send()
            .await
            .map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        Ok(Box::pin(
            response
                .bytes_stream()
                .map(|res| res.map_err(payload_to_io_error)),
        ))
    }

    #[tracing::instrument(skip(self, writer))]
    async fn read_into<Writer>(
        &self,
        identifier: &Arc<str>,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Unpin,
    {
        let response = self
            .get_object_request(identifier, None, None)
            .send()
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, ObjectError::from(e)))?;

        if !response.status().is_success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                status_error(response).await,
            ));
        }

        let mut stream = response.bytes_stream().into_streamer();

        while let Some(res) = stream.next().await {
            let mut bytes = res.map_err(payload_to_io_error)?;
            writer.write_all_buf(&mut bytes).await?;
        }
        writer.flush().await?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError> {
        let response = self
            .head_object_request(identifier)
            .send()
            .await
            .map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        let length = response
            .headers()
            .get(CONTENT_LENGTH)
            .ok_or(ObjectError::Length)?
            .to_str()
            .map_err(|_| ObjectError::Length)?
            .parse::<u64>()
            .map_err(|_| ObjectError::Length)?;

        Ok(length)
    }

    #[tracing::instrument(skip(self))]
    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError> {
        let response = self
            .delete_object_request(identifier)
            .send()
            .await
            .map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        Ok(())
    }
}

impl ObjectStore {
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(skip(access_key, secret_key, session_token, repo))]
    pub(crate) async fn build(
        endpoint: Url,
        bucket_name: String,
        url_style: UrlStyle,
        region: String,
        access_key: String,
        secret_key: String,
        session_token: Option<String>,
        signature_expiration: u64,
        client_timeout: u64,
        public_endpoint: Option<Url>,
        repo: ArcRepo,
    ) -> Result<ObjectStoreConfig, StoreError> {
        let path_gen = init_generator(&repo).await?;

        Ok(ObjectStoreConfig {
            path_gen,
            repo,
            bucket: Bucket::new(endpoint, url_style, bucket_name, region)
                .map_err(ObjectError::from)?,
            credentials: if let Some(token) = session_token {
                Credentials::new_with_token(access_key, secret_key, token)
            } else {
                Credentials::new(access_key, secret_key)
            },
            signature_expiration,
            client_timeout,
            public_endpoint,
        })
    }

    async fn head_bucket_request(&self) -> Result<RequestBuilder, StoreError> {
        let action = self.bucket.head_bucket(Some(&self.credentials));

        Ok(self.build_request(action))
    }

    async fn put_object_request(
        &self,
        length: usize,
        content_type: mime::Mime,
    ) -> Result<(RequestBuilder, Arc<str>), StoreError> {
        let path = self.next_file().await?;

        let mut action = self.bucket.put_object(Some(&self.credentials), &path);

        action
            .headers_mut()
            .insert("content-type", content_type.as_ref());
        action
            .headers_mut()
            .insert("content-length", length.to_string());

        Ok((self.build_request(action), Arc::from(path)))
    }

    async fn create_multipart_request(
        &self,
        content_type: mime::Mime,
    ) -> Result<(RequestBuilder, Arc<str>), StoreError> {
        let path = self.next_file().await?;

        let mut action = self
            .bucket
            .create_multipart_upload(Some(&self.credentials), &path);

        action
            .headers_mut()
            .insert("content-type", content_type.as_ref());

        Ok((self.build_request(action), Arc::from(path)))
    }

    async fn create_upload_part_request(
        &self,
        buf: BytesStream,
        object_id: &Arc<str>,
        part_number: u16,
        upload_id: &str,
    ) -> Result<RequestBuilder, ObjectError> {
        use md5::Digest;

        let mut action = self.bucket.upload_part(
            Some(&self.credentials),
            object_id.as_ref(),
            part_number,
            upload_id,
        );

        let length = buf.len();

        let hashing_span = tracing::info_span!("Hashing request body");
        let hash_string = actix_web::web::block(move || {
            let guard = hashing_span.enter();
            let mut hasher = md5::Md5::new();
            for bytes in buf {
                hasher.update(&bytes);
            }
            let hash = hasher.finalize();
            let hash_string = BASE64_STANDARD.encode(hash);
            drop(guard);
            hash_string
        })
        .await
        .map_err(ObjectError::from)?;

        action
            .headers_mut()
            .insert("content-type", "application/octet-stream");
        action.headers_mut().insert("content-md5", hash_string);
        action
            .headers_mut()
            .insert("content-length", length.to_string());

        Ok(self.build_request(action))
    }

    async fn send_complete_multipart_request<'a, I: Iterator<Item = &'a str>>(
        &'a self,
        object_id: &'a Arc<str>,
        upload_id: &'a str,
        etags: I,
    ) -> Result<Response, reqwest_middleware::Error> {
        let mut action = self.bucket.complete_multipart_upload(
            Some(&self.credentials),
            object_id.as_ref(),
            upload_id,
            etags,
        );

        action
            .headers_mut()
            .insert("content-type", "application/octet-stream");

        let (req, action) = self.build_request_inner(action);

        let body: Vec<u8> = action.body().into();

        req.header(CONTENT_LENGTH, body.len())
            .body(body)
            .send()
            .await
    }

    fn create_abort_multipart_request(
        &self,
        object_id: &Arc<str>,
        upload_id: &str,
    ) -> RequestBuilder {
        let action = self.bucket.abort_multipart_upload(
            Some(&self.credentials),
            object_id.as_ref(),
            upload_id,
        );

        self.build_request(action)
    }

    fn build_request<'a, A: S3Action<'a>>(&'a self, action: A) -> RequestBuilder {
        let (req, _) = self.build_request_inner(action);
        req
    }

    fn build_request_inner<'a, A: S3Action<'a>>(&'a self, mut action: A) -> (RequestBuilder, A) {
        let method = match A::METHOD {
            rusty_s3::Method::Head => reqwest::Method::HEAD,
            rusty_s3::Method::Get => reqwest::Method::GET,
            rusty_s3::Method::Post => reqwest::Method::POST,
            rusty_s3::Method::Put => reqwest::Method::PUT,
            rusty_s3::Method::Delete => reqwest::Method::DELETE,
        };

        let url = action.sign(self.signature_expiration);

        let req = self
            .client
            .request(method, url.as_str())
            .timeout(self.client_timeout);

        let req = action
            .headers_mut()
            .iter()
            .fold(req, |req, (name, value)| req.header(name, value));

        (req, action)
    }

    fn get_object_request(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> RequestBuilder {
        let action = self
            .bucket
            .get_object(Some(&self.credentials), identifier.as_ref());

        let req = self.build_request(action);

        let start = from_start.unwrap_or(0);
        let end = len.map(|len| start + len - 1);

        req.header(
            RANGE,
            Range::Bytes(vec![if let Some(end) = end {
                ByteRangeSpec::FromTo(start, end)
            } else {
                ByteRangeSpec::From(start)
            }])
            .to_string(),
        )
    }

    fn head_object_request(&self, identifier: &Arc<str>) -> RequestBuilder {
        let action = self
            .bucket
            .head_object(Some(&self.credentials), identifier.as_ref());

        self.build_request(action)
    }

    fn delete_object_request(&self, identifier: &Arc<str>) -> RequestBuilder {
        let action = self
            .bucket
            .delete_object(Some(&self.credentials), identifier.as_ref());

        self.build_request(action)
    }

    async fn next_directory(&self) -> Result<Path, StoreError> {
        let path = self.path_gen.next();

        self.repo
            .set(GENERATOR_KEY, path.to_be_bytes().into())
            .await?;

        Ok(path)
    }

    async fn next_file(&self) -> Result<String, StoreError> {
        let path = self.next_directory().await?.to_strings().join("/");
        let filename = uuid::Uuid::new_v4().to_string();

        Ok(format!("{path}/{filename}"))
    }
}

async fn init_generator(repo: &ArcRepo) -> Result<Generator, StoreError> {
    if let Some(ivec) = repo.get(GENERATOR_KEY).await? {
        Ok(Generator::from_existing(
            storage_path_generator::Path::from_be_bytes(ivec.to_vec())
                .map_err(ObjectError::from)?,
        ))
    } else {
        Ok(Generator::new())
    }
}

impl std::fmt::Debug for ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStore")
            .field("path_gen", &"generator")
            .field("bucket", &self.bucket.name())
            .field("region", &self.bucket.region())
            .finish()
    }
}
