use crate::{
    bytes_stream::BytesStream,
    error::Error,
    repo::{Repo, SettingsRepo},
    store::{Store, StoreConfig},
};
use actix_rt::task::JoinError;
use actix_web::{
    error::{BlockingError, PayloadError},
    http::{
        header::{ByteRangeSpec, Range, CONTENT_LENGTH},
        StatusCode,
    },
    web::Bytes,
};
use awc::{error::SendRequestError, Client, ClientRequest, ClientResponse, SendClientRequest};
use futures_util::{Stream, StreamExt, TryStreamExt};
use rusty_s3::{actions::S3Action, Bucket, BucketError, Credentials, UrlStyle};
use std::{pin::Pin, string::FromUtf8Error, time::Duration};
use storage_path_generator::{Generator, Path};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tracing::Instrument;
use url::Url;

mod object_id;
pub(crate) use object_id::ObjectId;

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

    #[error("Error making request")]
    SendRequest(String),

    #[error("Failed to parse string")]
    Utf8(#[from] FromUtf8Error),

    #[error("Failed to parse xml")]
    Xml(#[from] quick_xml::de::DeError),

    #[error("Invalid length")]
    Length,

    #[error("Invalid etag response")]
    Etag,

    #[error("Task cancelled")]
    Cancelled,

    #[error("Invalid status: {0}\n{1}")]
    Status(StatusCode, String),
}

impl From<SendRequestError> for ObjectError {
    fn from(e: SendRequestError) -> Self {
        Self::SendRequest(e.to_string())
    }
}

impl From<JoinError> for ObjectError {
    fn from(_: JoinError) -> Self {
        Self::Cancelled
    }
}

impl From<BlockingError> for ObjectError {
    fn from(_: BlockingError) -> Self {
        Self::Cancelled
    }
}

#[derive(Clone)]
pub(crate) struct ObjectStore {
    path_gen: Generator,
    repo: Repo,
    bucket: Bucket,
    credentials: Credentials,
    client: Client,
}

#[derive(Clone)]
pub(crate) struct ObjectStoreConfig {
    path_gen: Generator,
    repo: Repo,
    bucket: Bucket,
    credentials: Credentials,
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

impl StoreConfig for ObjectStoreConfig {
    type Store = ObjectStore;

    fn build(self) -> Self::Store {
        ObjectStore {
            path_gen: self.path_gen,
            repo: self.repo,
            bucket: self.bucket,
            credentials: self.credentials,
            client: crate::build_client(),
        }
    }
}

fn payload_to_io_error(e: PayloadError) -> std::io::Error {
    match e {
        PayloadError::Io(io) => io,
        otherwise => std::io::Error::new(std::io::ErrorKind::Other, otherwise.to_string()),
    }
}

#[tracing::instrument(skip(stream))]
async fn read_chunk<S>(stream: &mut S) -> std::io::Result<BytesStream>
where
    S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
{
    let mut buf = BytesStream::new();

    while buf.len() < CHUNK_SIZE {
        if let Some(res) = stream.next().await {
            buf.add_bytes(res?)
        } else {
            break;
        }
    }

    Ok(buf)
}

async fn status_error(mut response: ClientResponse) -> Error {
    let body = match response.body().await {
        Err(e) => return e.into(),
        Ok(body) => body,
    };

    let body = String::from_utf8_lossy(&body).to_string();

    ObjectError::Status(response.status(), body).into()
}

#[async_trait::async_trait(?Send)]
impl Store for ObjectStore {
    type Identifier = ObjectId;
    type Stream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>>>>;

    #[tracing::instrument(skip(reader))]
    async fn save_async_read<Reader>(&self, reader: Reader) -> Result<Self::Identifier, Error>
    where
        Reader: AsyncRead + Unpin + 'static,
    {
        self.save_stream(ReaderStream::new(reader)).await
    }

    #[tracing::instrument(skip(stream))]
    async fn save_stream<S>(&self, mut stream: S) -> Result<Self::Identifier, Error>
    where
        S: Stream<Item = std::io::Result<Bytes>> + Unpin + 'static,
    {
        let first_chunk = read_chunk(&mut stream).await?;

        if first_chunk.len() < CHUNK_SIZE {
            drop(stream);
            let (req, object_id) = self.put_object_request().await?;
            let response = req.send_body(first_chunk).await?;

            if !response.status().is_success() {
                return Err(status_error(response).await);
            }

            return Ok(object_id);
        }

        let mut first_chunk = Some(first_chunk);

        let (req, object_id) = self.create_multipart_request().await?;
        let mut response = req.send().await.map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        let body = response.body().await?;
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
                let handle = actix_rt::spawn(
                    async move {
                        let response = this
                            .create_upload_part_request(
                                buf.clone(),
                                &object_id2,
                                part_number,
                                &upload_id2,
                            )
                            .await?
                            .send_body(buf)
                            .await?;

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

                        Ok(etag) as Result<String, Error>
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
                .await?;

            if !response.status().is_success() {
                return Err(status_error(response).await);
            }

            Ok(()) as Result<(), Error>
        }
        .await;

        if let Err(e) = res {
            self.create_abort_multipart_request(&object_id, upload_id)
                .send()
                .await?;
            return Err(e);
        }

        Ok(object_id)
    }

    #[tracing::instrument(skip(bytes))]
    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, Error> {
        let (req, object_id) = self.put_object_request().await?;

        let response = req.send_body(bytes).await.map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        Ok(object_id)
    }

    #[tracing::instrument]
    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, Error> {
        let response = self
            .get_object_request(identifier, from_start, len)
            .send()
            .await
            .map_err(ObjectError::from)?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        Ok(Box::pin(response.map_err(payload_to_io_error)))
    }

    #[tracing::instrument(skip(writer))]
    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Unpin,
    {
        let mut response = self
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

        while let Some(res) = response.next().await {
            let mut bytes = res.map_err(payload_to_io_error)?;
            writer.write_all_buf(&mut bytes).await?;
        }
        writer.flush().await?;

        Ok(())
    }

    #[tracing::instrument]
    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Error> {
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

    #[tracing::instrument]
    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Error> {
        let response = self.delete_object_request(identifier).send().await?;

        if !response.status().is_success() {
            return Err(status_error(response).await);
        }

        Ok(())
    }
}

impl ObjectStore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn build(
        endpoint: Url,
        bucket_name: String,
        url_style: UrlStyle,
        region: String,
        access_key: String,
        secret_key: String,
        session_token: Option<String>,
        repo: Repo,
    ) -> Result<ObjectStoreConfig, Error> {
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
        })
    }

    async fn put_object_request(&self) -> Result<(ClientRequest, ObjectId), Error> {
        let path = self.next_file().await?;

        let mut action = self.bucket.put_object(Some(&self.credentials), &path);

        action
            .headers_mut()
            .insert("content-type", "application/octet-stream");

        Ok((self.build_request(action), ObjectId::from_string(path)))
    }

    async fn create_multipart_request(&self) -> Result<(ClientRequest, ObjectId), Error> {
        let path = self.next_file().await?;

        let mut action = self
            .bucket
            .create_multipart_upload(Some(&self.credentials), &path);

        action
            .headers_mut()
            .insert("content-type", "application/octet-stream");

        Ok((self.build_request(action), ObjectId::from_string(path)))
    }

    async fn create_upload_part_request(
        &self,
        buf: BytesStream,
        object_id: &ObjectId,
        part_number: u16,
        upload_id: &str,
    ) -> Result<ClientRequest, Error> {
        use md5::Digest;

        let mut action = self.bucket.upload_part(
            Some(&self.credentials),
            object_id.as_str(),
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
            let hash_string = base64::encode(&hash);
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

    fn send_complete_multipart_request<'a, I: Iterator<Item = &'a str>>(
        &'a self,
        object_id: &'a ObjectId,
        upload_id: &'a str,
        etags: I,
    ) -> SendClientRequest {
        let mut action = self.bucket.complete_multipart_upload(
            Some(&self.credentials),
            object_id.as_str(),
            upload_id,
            etags,
        );

        action
            .headers_mut()
            .insert("content-type", "application/octet-stream");

        let (req, action) = self.build_request_inner(action);

        req.send_body(action.body())
    }

    fn create_abort_multipart_request(
        &self,
        object_id: &ObjectId,
        upload_id: &str,
    ) -> ClientRequest {
        let action = self.bucket.abort_multipart_upload(
            Some(&self.credentials),
            object_id.as_str(),
            upload_id,
        );

        self.build_request(action)
    }

    fn build_request<'a, A: S3Action<'a>>(&'a self, action: A) -> ClientRequest {
        let (req, _) = self.build_request_inner(action);
        req
    }

    fn build_request_inner<'a, A: S3Action<'a>>(&'a self, mut action: A) -> (ClientRequest, A) {
        let method = match A::METHOD {
            rusty_s3::Method::Head => awc::http::Method::HEAD,
            rusty_s3::Method::Get => awc::http::Method::GET,
            rusty_s3::Method::Post => awc::http::Method::POST,
            rusty_s3::Method::Put => awc::http::Method::PUT,
            rusty_s3::Method::Delete => awc::http::Method::DELETE,
        };

        let url = action.sign(Duration::from_secs(5));

        let req = self.client.request(method, url.as_str());

        let req = action
            .headers_mut()
            .iter()
            .fold(req, |req, tup| req.insert_header(tup));

        (req, action)
    }

    fn get_object_request(
        &self,
        identifier: &ObjectId,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> ClientRequest {
        let action = self
            .bucket
            .get_object(Some(&self.credentials), identifier.as_str());

        let req = self.build_request(action);

        let start = from_start.unwrap_or(0);
        let end = len.map(|len| start + len - 1);

        req.insert_header(Range::Bytes(vec![if let Some(end) = end {
            ByteRangeSpec::FromTo(start, end)
        } else {
            ByteRangeSpec::From(start)
        }]))
    }

    fn head_object_request(&self, identifier: &ObjectId) -> ClientRequest {
        let action = self
            .bucket
            .head_object(Some(&self.credentials), identifier.as_str());

        self.build_request(action)
    }

    fn delete_object_request(&self, identifier: &ObjectId) -> ClientRequest {
        let action = self
            .bucket
            .delete_object(Some(&self.credentials), identifier.as_str());

        self.build_request(action)
    }

    async fn next_directory(&self) -> Result<Path, Error> {
        let path = self.path_gen.next();

        match self.repo {
            Repo::Sled(ref sled_repo) => {
                sled_repo
                    .set(GENERATOR_KEY, path.to_be_bytes().into())
                    .await?;
            }
        }

        Ok(path)
    }

    async fn next_file(&self) -> Result<String, Error> {
        let path = self.next_directory().await?.to_strings().join("/");
        let filename = uuid::Uuid::new_v4().to_string();

        Ok(format!("{}/{}", path, filename))
    }
}

async fn init_generator(repo: &Repo) -> Result<Generator, Error> {
    match repo {
        Repo::Sled(sled_repo) => {
            if let Some(ivec) = sled_repo.get(GENERATOR_KEY).await? {
                Ok(Generator::from_existing(
                    storage_path_generator::Path::from_be_bytes(ivec.to_vec())?,
                ))
            } else {
                Ok(Generator::new())
            }
        }
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
