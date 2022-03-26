use crate::{
    repo::{Repo, SettingsRepo},
    store::Store,
};
use actix_web::web::Bytes;
use futures_util::stream::Stream;
use s3::{
    client::Client, command::Command, creds::Credentials, request_trait::Request, Bucket, Region,
};
use std::{
    pin::Pin,
    string::FromUtf8Error,
    task::{Context, Poll},
};
use storage_path_generator::{Generator, Path};
use tokio::io::{AsyncRead, AsyncWrite};

mod object_id;
pub(crate) use object_id::ObjectId;

// - Settings Tree
//   - last-path -> last generated path

const GENERATOR_KEY: &[u8] = b"last-path";

#[derive(Debug, thiserror::Error)]
pub(crate) enum ObjectError {
    #[error("Failed to generate path")]
    PathGenerator(#[from] storage_path_generator::PathError),

    #[error("Failed to interact with sled repo")]
    Sled(#[from] crate::repo::sled::Error),

    #[error("Failed to parse string")]
    Utf8(#[from] FromUtf8Error),

    #[error("Invalid length")]
    Length,

    #[error("Storage error")]
    Anyhow(#[from] anyhow::Error),
}

#[derive(Clone)]
pub(crate) struct ObjectStore {
    path_gen: Generator,
    repo: Repo,
    bucket: Bucket,
    client: reqwest::Client,
}

pin_project_lite::pin_project! {
    struct IoError<S> {
        #[pin]
        inner: S,
    }
}

#[async_trait::async_trait(?Send)]
impl Store for ObjectStore {
    type Error = ObjectError;
    type Identifier = ObjectId;
    type Stream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>>>>;

    #[tracing::instrument(skip(reader))]
    async fn save_async_read<Reader>(
        &self,
        reader: &mut Reader,
    ) -> Result<Self::Identifier, Self::Error>
    where
        Reader: AsyncRead + Unpin,
    {
        let path = self.next_file().await?;

        self.bucket
            .put_object_stream(&self.client, reader, &path)
            .await?;

        Ok(ObjectId::from_string(path))
    }

    #[tracing::instrument(skip(bytes))]
    async fn save_bytes(&self, bytes: Bytes) -> Result<Self::Identifier, Self::Error> {
        let path = self.next_file().await?;

        self.bucket.put_object(&self.client, &path, &bytes).await?;

        Ok(ObjectId::from_string(path))
    }

    #[tracing::instrument]
    async fn to_stream(
        &self,
        identifier: &Self::Identifier,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<Self::Stream, Self::Error> {
        let path = identifier.as_str();

        let start = from_start.unwrap_or(0);
        let end = len.map(|len| start + len);

        let request = Client::request(
            &self.client,
            &self.bucket,
            path,
            Command::GetObjectRange { start, end },
        );

        let response = request.response().await?;

        Ok(Box::pin(io_error(response.bytes_stream())))
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
        let path = identifier.as_str();

        self.bucket
            .get_object_stream(&self.client, path, writer)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, Self::Error::from(e)))?;

        Ok(())
    }

    #[tracing::instrument]
    async fn len(&self, identifier: &Self::Identifier) -> Result<u64, Self::Error> {
        let path = identifier.as_str();

        let (head, _) = self.bucket.head_object(&self.client, path).await?;
        let length = head.content_length.ok_or(ObjectError::Length)?;

        Ok(length as u64)
    }

    #[tracing::instrument]
    async fn remove(&self, identifier: &Self::Identifier) -> Result<(), Self::Error> {
        let path = identifier.as_str();

        self.bucket.delete_object(&self.client, path).await?;
        Ok(())
    }
}

impl ObjectStore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn build(
        bucket_name: &str,
        region: Region,
        access_key: Option<String>,
        secret_key: Option<String>,
        security_token: Option<String>,
        session_token: Option<String>,
        repo: Repo,
        client: reqwest::Client,
    ) -> Result<ObjectStore, ObjectError> {
        let path_gen = init_generator(&repo).await?;

        Ok(ObjectStore {
            path_gen,
            repo,
            bucket: Bucket::new_with_path_style(
                bucket_name,
                match region {
                    Region::Custom { endpoint, .. } => Region::Custom {
                        region: String::from(""),
                        endpoint,
                    },
                    region => region,
                },
                Credentials {
                    access_key,
                    secret_key,
                    security_token,
                    session_token,
                },
            )?,
            client,
        })
    }

    async fn next_directory(&self) -> Result<Path, ObjectError> {
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

    async fn next_file(&self) -> Result<String, ObjectError> {
        let path = self.next_directory().await?.to_strings().join("/");
        let filename = uuid::Uuid::new_v4().to_string();

        Ok(format!("{}/{}", path, filename))
    }
}

async fn init_generator(repo: &Repo) -> Result<Generator, ObjectError> {
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

fn io_error<S, T, E>(stream: S) -> impl Stream<Item = std::io::Result<T>>
where
    S: Stream<Item = Result<T, E>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    IoError { inner: stream }
}

impl<S, T, E> Stream for IoError<S>
where
    S: Stream<Item = Result<T, E>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = std::io::Result<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().project();

        this.inner.poll_next(cx).map(|opt| {
            opt.map(|res| res.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)))
        })
    }
}

impl std::fmt::Debug for ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStore")
            .field("path_gen", &self.path_gen)
            .field("bucket", &self.bucket.name)
            .field("region", &self.bucket.region)
            .finish()
    }
}
