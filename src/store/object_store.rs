use crate::{
    bytes_stream::BytesStream, error_code::ErrorCode, future::WithMetrics, store::Store,
    stream::LocalBoxStream,
};
use actix_web::{rt::task::JoinError, web::Bytes};
use futures_core::Stream;
use object_store::{
    aws::{AmazonS3, AmazonS3Builder},
    path::Path,
    Attribute, AttributeValue, Attributes, GetOptions, ObjectStore as ObjectStoreTrait, PutMode,
    PutMultipartOpts, PutOptions, PutPayload, PutPayloadMut,
};
use std::{sync::Arc, time::Duration};
use streem::IntoStreamer;
use url::Url;

use super::StoreError;

const CHUNK_SIZE: usize = 8_388_608; // 8 Mebibytes, min is 5 (5_242_880);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ObjectError {
    #[error("Failed to set the vhost-style bucket name")]
    SetHost,

    #[error("IO Error")]
    IO(#[from] std::io::Error),

    #[error("Error in request response")]
    Request(#[from] object_store::Error),

    #[error("Failed to build object store client")]
    BuildClient(#[source] object_store::Error),

    #[error("Task cancelled")]
    Canceled,
}

impl ObjectError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::SetHost | Self::BuildClient(_) | Self::Request(_) => {
                ErrorCode::OBJECT_REQUEST_ERROR
            }
            Self::IO(_) => ErrorCode::OBJECT_IO_ERROR,
            Self::Canceled => ErrorCode::PANIC,
        }
    }
}

impl From<JoinError> for ObjectError {
    fn from(_: JoinError) -> Self {
        Self::Canceled
    }
}

#[derive(Clone)]
pub(crate) struct ObjectStore {
    s3_client: Arc<AmazonS3>,
    public_endpoint: Option<Url>,
}

#[tracing::instrument(level = "debug", skip(stream))]
async fn read_chunk<S>(stream: &mut S) -> Result<BytesStream, ObjectError>
where
    S: Stream<Item = std::io::Result<Bytes>> + Unpin,
{
    let mut buf = BytesStream::new();

    let mut stream = stream.into_streamer();

    while buf.len() < CHUNK_SIZE {
        tracing::trace!("read_chunk: looping");

        if let Some(bytes) = stream.try_next().await? {
            buf.add_bytes(bytes)
        } else {
            break;
        }
    }

    tracing::debug!(
        "BytesStream with {} chunks, avg length {}",
        buf.chunks_len(),
        buf.len() / buf.chunks_len().max(1)
    );

    Ok(buf)
}

impl From<BytesStream> for PutPayload {
    fn from(value: BytesStream) -> Self {
        let mut payload = PutPayloadMut::new();

        for bytes in value {
            payload.push(bytes);
        }

        payload.freeze()
    }
}

impl Store for ObjectStore {
    async fn health_check(&self) -> Result<(), StoreError> {
        let res = self
            .s3_client
            .put_opts(
                &Path::from("health-check"),
                PutPayload::new(),
                PutOptions {
                    mode: PutMode::Overwrite,
                    ..Default::default()
                },
            )
            .with_metrics(crate::init_metrics::OBJECT_STORAGE_PUT_OBJECT_REQUEST)
            .await;

        match res {
            Ok(_) => Ok(()),
            Err(object_store::Error::NotModified { .. }) => Ok(()),
            Err(e) => Err(ObjectError::Request(e).into()),
        }
    }

    #[tracing::instrument(skip_all)]
    async fn save_stream<S>(
        &self,
        stream: S,
        content_type: mime::Mime,
        extension: Option<&str>,
    ) -> Result<Arc<str>, StoreError>
    where
        S: Stream<Item = std::io::Result<Bytes>>,
    {
        let mut stream = std::pin::pin!(stream);
        let first_chunk = read_chunk(&mut stream).await?;

        let object_id: Arc<str> = Arc::from(self.next_file(extension));
        let path = Path::from(object_id.as_ref());

        let mut attributes = Attributes::new();
        attributes.insert(
            Attribute::ContentType,
            AttributeValue::from(content_type.to_string()),
        );

        if first_chunk.len() < CHUNK_SIZE {
            self.s3_client
                .put_opts(
                    &path,
                    first_chunk.into(),
                    PutOptions {
                        attributes,
                        ..Default::default()
                    },
                )
                .with_metrics(crate::init_metrics::OBJECT_STORAGE_PUT_OBJECT_REQUEST)
                .await
                .map_err(ObjectError::from)?;

            return Ok(object_id);
        }

        let mut multipart = self
            .s3_client
            .put_multipart_opts(
                &path,
                PutMultipartOpts {
                    attributes,
                    ..Default::default()
                },
            )
            .with_metrics(crate::init_metrics::OBJECT_STORAGE_CREATE_MULTIPART_REQUEST)
            .await
            .map_err(ObjectError::from)?;

        let res = async {
            let mut first_chunk = Some(first_chunk);
            let mut complete = false;

            let mut futures = Vec::new();

            while !complete {
                let buf = if let Some(chunk) = first_chunk.take() {
                    chunk
                } else {
                    read_chunk(&mut stream).await?
                };

                complete = buf.len() < CHUNK_SIZE;

                futures.push(crate::sync::abort_on_drop(crate::sync::spawn(
                    "put-multipart-part",
                    multipart.put_part(buf.into()).with_metrics(
                        crate::init_metrics::OBJECT_STORAGE_CREATE_UPLOAD_PART_REQUEST,
                    ),
                )));
            }

            for future in futures {
                future
                    .await
                    .map_err(ObjectError::from)?
                    .map_err(ObjectError::from)?;
            }

            multipart
                .complete()
                .with_metrics(crate::init_metrics::OBJECT_STORAGE_COMPLETE_MULTIPART_REQUEST)
                .await
                .map_err(ObjectError::from)?;

            Ok(())
        }
        .await;

        match res {
            Ok(()) => Ok(object_id),
            Err(e) => {
                multipart
                    .abort()
                    .with_metrics(crate::init_metrics::OBJECT_STORAGE_ABORT_MULTIPART_REQUEST)
                    .await
                    .map_err(ObjectError::from)?;
                Err(e)
            }
        }
    }

    fn public_url(&self, identifier: &Arc<str>) -> Option<url::Url> {
        self.public_endpoint.clone().and_then(|mut endpoint| {
            endpoint
                .path_segments_mut()
                .ok()?
                .pop_if_empty()
                .extend(identifier.as_ref().split('/'));
            Some(endpoint)
        })
    }

    #[tracing::instrument(skip(self))]
    async fn to_stream(
        &self,
        identifier: &Arc<str>,
        from_start: Option<u64>,
        len: Option<u64>,
    ) -> Result<LocalBoxStream<'static, std::io::Result<Bytes>>, StoreError> {
        let from_start = from_start.map(|u| u as usize);
        let len = len.map(|u| u as usize);

        let range = match (from_start, len) {
            (Some(start), Some(length)) => Some((start..start + length).into()),
            (Some(start), None) => Some((start..).into()),
            (None, Some(length)) => Some((..length).into()),
            (None, None) => None,
        };

        let path = Path::from(identifier.as_ref());

        let get_result = self
            .s3_client
            .get_opts(
                &path,
                GetOptions {
                    range,
                    ..Default::default()
                },
            )
            .with_metrics(crate::init_metrics::OBJECT_STORAGE_GET_OBJECT_REQUEST)
            .await
            .map_err(ObjectError::from)?;

        Ok(Box::pin(crate::stream::metrics(
            crate::init_metrics::OBJECT_STORAGE_GET_OBJECT_REQUEST_STREAM,
            crate::stream::map_err(get_result.into_stream(), |e| {
                std::io::Error::new(std::io::ErrorKind::Other, e)
            }),
        )))
    }

    #[tracing::instrument(skip(self))]
    async fn len(&self, identifier: &Arc<str>) -> Result<u64, StoreError> {
        let path = Path::from(identifier.as_ref());

        let object_meta = self
            .s3_client
            .head(&path)
            .with_metrics(crate::init_metrics::OBJECT_STORAGE_HEAD_OBJECT_REQUEST)
            .await
            .map_err(ObjectError::from)?;

        Ok(object_meta.size as _)
    }

    #[tracing::instrument(skip(self))]
    async fn remove(&self, identifier: &Arc<str>) -> Result<(), StoreError> {
        let path = Path::from(identifier.as_ref());

        self.s3_client
            .delete(&path)
            .with_metrics(crate::init_metrics::OBJECT_STORAGE_DELETE_OBJECT_REQUEST)
            .await
            .map_err(ObjectError::from)?;

        Ok(())
    }
}

impl ObjectStore {
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(skip(access_key, secret_key, session_token))]
    pub(crate) async fn new(
        crate::config::ObjectStorage {
            mut endpoint,
            bucket_name,
            use_path_style,
            region,
            access_key,
            secret_key,
            session_token,
            client_timeout,
            public_endpoint,
        }: crate::config::ObjectStorage,
    ) -> Result<ObjectStore, StoreError> {
        let https = endpoint.scheme() == "https";

        let client_options = object_store::ClientOptions::new()
            .with_timeout(Duration::from_secs(client_timeout))
            .with_allow_http(!https);

        let use_vhost_style = !use_path_style;

        if use_vhost_style {
            if let Some(host) = endpoint.host() {
                if !host.to_string().starts_with(&bucket_name) {
                    let new_host = format!("{bucket_name}.{host}");
                    endpoint
                        .set_host(Some(&new_host))
                        .map_err(|_| ObjectError::SetHost)?;
                }
            }
        }

        let builder = AmazonS3Builder::new()
            .with_endpoint(endpoint.as_str().trim_end_matches('/'))
            .with_bucket_name(bucket_name)
            .with_virtual_hosted_style_request(use_vhost_style)
            .with_region(region)
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key)
            .with_allow_http(!https)
            .with_client_options(client_options);

        let builder = if let Some(token) = session_token {
            builder.with_token(token)
        } else {
            builder
        };

        let s3_client = builder.build().map_err(ObjectError::BuildClient)?;

        Ok(ObjectStore {
            s3_client: Arc::new(s3_client),
            public_endpoint,
        })
    }

    fn next_file(&self, extension: Option<&str>) -> String {
        crate::file_path::generate_object(extension)
    }
}

impl std::fmt::Debug for ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStore").finish()
    }
}
