use crate::{
    error::Error,
    repo::{Repo, SettingsRepo},
    store::Store,
};
use actix_web::{
    http::{
        header::{ByteRangeSpec, Range, CONTENT_LENGTH},
        StatusCode,
    },
    web::Bytes,
};
use awc::{Client, ClientRequest};
use futures_util::{Stream, TryStreamExt};
use rusty_s3::{actions::S3Action, Bucket, Credentials, UrlStyle};
use std::{pin::Pin, string::FromUtf8Error, time::Duration};
use storage_path_generator::{Generator, Path};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use url::Url;

mod object_id;
pub(crate) use object_id::ObjectId;

// - Settings Tree
//   - last-path -> last generated path

const GENERATOR_KEY: &str = "last-path";

#[derive(Debug, thiserror::Error)]
pub(crate) enum ObjectError {
    #[error("Failed to generate path")]
    PathGenerator(#[from] storage_path_generator::PathError),

    #[error("Failed to parse string")]
    Utf8(#[from] FromUtf8Error),

    #[error("Invalid length")]
    Length,

    #[error("Invalid status")]
    Status(StatusCode),
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

#[async_trait::async_trait(?Send)]
impl Store for ObjectStore {
    type Config = ObjectStoreConfig;
    type Identifier = ObjectId;
    type Stream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>>>>;

    fn init(config: Self::Config) -> Self {
        ObjectStore {
            path_gen: config.path_gen,
            repo: config.repo,
            bucket: config.bucket,
            credentials: config.credentials,
            client: crate::build_client(),
        }
    }

    #[tracing::instrument(skip(reader))]
    async fn save_async_read<Reader>(&self, reader: &mut Reader) -> Result<Self::Identifier, Error>
    where
        Reader: AsyncRead + Unpin,
    {
        let (req, object_id) = self.put_object_request().await?;

        let response = req.send_stream(ReaderStream::new(reader)).await?;

        if response.status().is_success() {
            return Ok(object_id);
        }

        Err(ObjectError::Status(response.status()).into())
    }

    #[tracing::instrument(skip(bytes))]
    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, Error> {
        let (req, object_id) = self.put_object_request().await?;

        let response = req.send_body(bytes).await?;

        if response.status().is_success() {
            return Ok(object_id);
        }

        Err(ObjectError::Status(response.status()).into())
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
            .await?;

        if response.status().is_success() {
            return Ok(Box::pin(response));
        }

        Err(ObjectError::Status(response.status()).into())
    }

    #[tracing::instrument(skip(writer))]
    async fn read_into<Writer>(
        &self,
        identifier: &Self::Identifier,
        writer: &mut Writer,
    ) -> Result<(), std::io::Error>
    where
        Writer: AsyncWrite + Send + Unpin,
    {
        let response = self
            .get_object_request(identifier, None, None)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ObjectError::Status(response.status()).into());
        }

        while let Some(res) = response.next().await {
            let bytes = res?;
            writer.write_all_buf(bytes).await?;
        }
        writer.flush().await?;

        Ok(())
    }

    #[tracing::instrument]
    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Error> {
        let response = self.head_object_request(identifier).send().await?;

        if !response.status().is_success() {
            return Err(ObjectError::Status(response.status()).into());
        }

        let length = response
            .headers()
            .get(CONTENT_LENGTH)
            .ok_or(ObjectError::Length)?
            .to_str()
            .ok_or(ObjectError::Length)
            .parse::<u64>()
            .map_err(|_| ObjectError::Length)?;

        Ok(length)
    }

    #[tracing::instrument]
    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Error> {
        let response = self.delete_object_request(identifier).send().await?;

        if !response.status().is_success() {
            return Err(ObjectError::Status(response.status()).into());
        }

        Ok(())
    }
}

impl ObjectStore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn build(
        endpoint: Url,
        bucket_name: &str,
        url_style: UrlStyle,
        region: &str,
        access_key: Option<String>,
        secret_key: Option<String>,
        session_token: Option<String>,
        repo: Repo,
    ) -> Result<ObjectStoreConfig, Error> {
        let path_gen = init_generator(&repo).await?;

        Ok(ObjectStoreConfig {
            path_gen,
            repo,
            bucket: Bucket::new(endpoint, url_style, bucket_name, region)
                .map_err(ObjectError::from)?,
            credentials: Credentials::new_with_token(access_key, secret_key, session_token),
        })
    }

    async fn put_object_request(&self) -> Result<(ClientRequest, ObjectId), Error> {
        let path = self.next_file().await?;

        let action = self.bucket.put_object(Some(&self.credentials), &path);

        Ok((self.build_request(action), ObjectId::from_string(path)))
    }

    fn build_request<'a, A: S3Action<'a>>(&'a self, action: A) -> ClientRequest {
        let method = match A::METHOD {
            rusty_s3::Method::Head => awc::http::Method::HEAD,
            rusty_s3::Method::Get => awc::http::Method::GET,
            rusty_s3::Method::Post => awc::http::Method::POST,
            rusty_s3::Method::Put => awc::http::Method::PUT,
            rusty_s3::Method::Delete => awc::http::Method::DELETE,
        };

        let url = action.sign(Duration::from_secs(5));

        let req = self.client.request(method, url);

        action
            .headers_mut()
            .drain()
            .fold(req, |req, tup| req.insert_header(tup))
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

        let range = match (start, end) {
            (Some(start), Some(end)) => Some(ByteRangeSpec::FromTo(start, end)),
            (Some(start), None) => Some(ByteRangeSpec::From(start)),
            _ => None,
        };

        if let Some(range) = range {
            req.insert_header(Range::Bytes(vec![range]))
        } else {
            req
        }
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
