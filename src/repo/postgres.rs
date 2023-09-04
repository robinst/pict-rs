mod embedded;
mod job_status;
mod schema;

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use dashmap::DashMap;
use diesel::prelude::*;
use diesel_async::{
    pooled_connection::{
        deadpool::{BuildError, Object, Pool, PoolError},
        AsyncDieselConnectionManager, ManagerConfig,
    },
    AsyncConnection, AsyncPgConnection, RunQueryDsl,
};
use tokio::sync::Notify;
use tokio_postgres::{AsyncMessage, Notification};
use url::Url;
use uuid::Uuid;

use crate::{
    details::Details,
    error_code::{ErrorCode, OwnedErrorCode},
    serde_str::Serde,
    stream::LocalBoxStream,
};

use self::job_status::JobStatus;

use super::{
    Alias, AliasAccessRepo, AliasAlreadyExists, AliasRepo, BaseRepo, DeleteToken, DetailsRepo,
    FullRepo, Hash, HashAlreadyExists, HashPage, HashRepo, JobId, OrderedHash, ProxyRepo,
    QueueRepo, RepoError, SettingsRepo, StoreMigrationRepo, UploadId, UploadRepo, UploadResult,
    VariantAccessRepo, VariantAlreadyExists,
};

#[derive(Clone)]
pub(crate) struct PostgresRepo {
    inner: Arc<Inner>,
    #[allow(dead_code)]
    notifications: Arc<actix_rt::task::JoinHandle<()>>,
}

struct Inner {
    health_count: AtomicU64,
    pool: Pool<AsyncPgConnection>,
    queue_notifications: DashMap<String, Arc<Notify>>,
    upload_notifier: Notify,
}

async fn delegate_notifications(receiver: flume::Receiver<Notification>, inner: Arc<Inner>) {
    while let Ok(notification) = receiver.recv_async().await {
        match notification.channel() {
            "queue_status_channel" => {
                // new job inserted for queue
                let queue_name = notification.payload().to_string();

                inner
                    .queue_notifications
                    .entry(queue_name)
                    .or_insert_with(|| Arc::new(Notify::new()))
                    .notify_waiters();
            }
            "upload_completion_channel" => {
                inner.upload_notifier.notify_waiters();
            }
            channel => {
                tracing::info!(
                    "Unhandled postgres notification: {channel}: {}",
                    notification.payload()
                );
            }
        }
    }

    tracing::warn!("Notification delegator shutting down");
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectPostgresError {
    #[error("Failed to connect to postgres for migrations")]
    ConnectForMigration(#[source] tokio_postgres::Error),

    #[error("Failed to run migrations")]
    Migration(#[source] refinery::Error),

    #[error("Failed to build postgres connection pool")]
    BuildPool(#[source] BuildError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PostgresError {
    #[error("Error in db pool")]
    Pool(#[source] PoolError),

    #[error("Error in database")]
    Diesel(#[source] diesel::result::Error),

    #[error("Error deserializing hex value")]
    Hex(#[source] hex::FromHexError),

    #[error("Error serializing details")]
    SerializeDetails(#[source] serde_json::Error),

    #[error("Error deserializing details")]
    DeserializeDetails(#[source] serde_json::Error),

    #[error("Error serializing upload result")]
    SerializeUploadResult(#[source] serde_json::Error),

    #[error("Error deserializing upload result")]
    DeserializeUploadResult(#[source] serde_json::Error),
}

impl PostgresError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Pool(_)
            | Self::Diesel(_)
            | Self::SerializeDetails(_)
            | Self::SerializeUploadResult(_)
            | Self::Hex(_) => ErrorCode::POSTGRES_ERROR,
            Self::DeserializeDetails(_) => ErrorCode::EXTRACT_DETAILS,
            Self::DeserializeUploadResult(_) => ErrorCode::EXTRACT_UPLOAD_RESULT,
        }
    }
}

impl PostgresRepo {
    pub(crate) async fn connect(postgres_url: Url) -> Result<Self, ConnectPostgresError> {
        let (mut client, conn) =
            tokio_postgres::connect(postgres_url.as_str(), tokio_postgres::tls::NoTls)
                .await
                .map_err(ConnectPostgresError::ConnectForMigration)?;

        let handle = actix_rt::spawn(conn);

        embedded::migrations::runner()
            .run_async(&mut client)
            .await
            .map_err(ConnectPostgresError::Migration)?;

        handle.abort();
        let _ = handle.await;

        let (tx, rx) = flume::bounded(10);

        let mut config = ManagerConfig::default();
        config.custom_setup = build_handler(tx);

        let mgr = AsyncDieselConnectionManager::<AsyncPgConnection>::new_with_config(
            postgres_url,
            config,
        );
        let pool = Pool::builder(mgr)
            .build()
            .map_err(ConnectPostgresError::BuildPool)?;

        let inner = Arc::new(Inner {
            health_count: AtomicU64::new(0),
            pool,
            queue_notifications: DashMap::new(),
            upload_notifier: Notify::new(),
        });

        let notifications = Arc::new(actix_rt::spawn(delegate_notifications(rx, inner.clone())));

        Ok(PostgresRepo {
            inner,
            notifications,
        })
    }

    async fn get_connection(&self) -> Result<Object<AsyncPgConnection>, PostgresError> {
        self.inner.get_connection().await
    }
}

impl Inner {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn get_connection(&self) -> Result<Object<AsyncPgConnection>, PostgresError> {
        self.pool.get().await.map_err(PostgresError::Pool)
    }
}

type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
type ConfigFn =
    Box<dyn Fn(&str) -> BoxFuture<'_, ConnectionResult<AsyncPgConnection>> + Send + Sync + 'static>;

fn build_handler(sender: flume::Sender<Notification>) -> ConfigFn {
    Box::new(
        move |config: &str| -> BoxFuture<'_, ConnectionResult<AsyncPgConnection>> {
            let sender = sender.clone();

            Box::pin(async move {
                let (client, mut conn) =
                    tokio_postgres::connect(config, tokio_postgres::tls::NoTls)
                        .await
                        .map_err(|e| ConnectionError::BadConnection(e.to_string()))?;

                // not very cash money (structured concurrency) of me
                actix_rt::spawn(async move {
                    while let Some(res) = std::future::poll_fn(|cx| conn.poll_message(cx)).await {
                        match res {
                            Err(e) => {
                                tracing::error!("Database Connection {e:?}");
                                return;
                            }
                            Ok(AsyncMessage::Notice(e)) => {
                                tracing::warn!("Database Notice {e:?}");
                            }
                            Ok(AsyncMessage::Notification(notification)) => {
                                if sender.send_async(notification).await.is_err() {
                                    tracing::warn!("Missed notification. Are we shutting down?");
                                }
                            }
                            Ok(_) => {
                                tracing::warn!("Unhandled AsyncMessage!!! Please contact the developer of this application");
                            }
                        }
                    }
                });

                AsyncPgConnection::try_from(client).await
            })
        },
    )
}

fn to_primitive(timestamp: time::OffsetDateTime) -> time::PrimitiveDateTime {
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    time::PrimitiveDateTime::new(timestamp.date(), timestamp.time())
}

impl BaseRepo for PostgresRepo {}

#[async_trait::async_trait(?Send)]
impl HashRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn size(&self) -> Result<u64, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let count = hashes
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(count.try_into().expect("non-negative count"))
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn bound(&self, input_hash: Hash) -> Result<Option<OrderedHash>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let timestamp = hashes
            .select(created_at)
            .filter(hash.eq(&input_hash))
            .first(&mut conn)
            .await
            .map(time::PrimitiveDateTime::assume_utc)
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(timestamp.map(|timestamp| OrderedHash {
            timestamp,
            hash: input_hash,
        }))
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn hash_page_by_date(
        &self,
        date: time::OffsetDateTime,
        limit: usize,
    ) -> Result<HashPage, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let timestamp = to_primitive(date);

        let ordered_hash = hashes
            .select((created_at, hash))
            .filter(created_at.lt(timestamp))
            .order(created_at.desc())
            .first::<(time::PrimitiveDateTime, Hash)>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(|tup| OrderedHash {
                timestamp: tup.0.assume_utc(),
                hash: tup.1,
            });

        self.hashes_ordered(ordered_hash, limit).await
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn hashes_ordered(
        &self,
        bound: Option<OrderedHash>,
        limit: usize,
    ) -> Result<HashPage, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let (mut page, prev) = if let Some(OrderedHash {
            timestamp,
            hash: bound_hash,
        }) = bound
        {
            let timestamp = to_primitive(timestamp);

            let page = hashes
                .select(hash)
                .filter(created_at.lt(timestamp))
                .or_filter(created_at.eq(timestamp).and(hash.le(&bound_hash)))
                .order(created_at.desc())
                .then_order_by(hash.desc())
                .limit(limit as i64 + 1)
                .load::<Hash>(&mut conn)
                .await
                .map_err(PostgresError::Diesel)?;

            let prev = hashes
                .select(hash)
                .filter(created_at.gt(timestamp))
                .or_filter(created_at.eq(timestamp).and(hash.gt(&bound_hash)))
                .order(created_at)
                .then_order_by(hash)
                .offset(limit.saturating_sub(1) as i64)
                .first::<Hash>(&mut conn)
                .await
                .optional()
                .map_err(PostgresError::Diesel)?;

            (page, prev)
        } else {
            let page = hashes
                .select(hash)
                .order(created_at.desc())
                .then_order_by(hash.desc())
                .limit(limit as i64 + 1)
                .load::<Hash>(&mut conn)
                .await
                .map_err(PostgresError::Diesel)?;

            (page, None)
        };

        let next = if page.len() > limit { page.pop() } else { None };

        Ok(HashPage {
            limit,
            prev,
            next,
            hashes: page,
        })
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn create_hash_with_timestamp(
        &self,
        input_hash: Hash,
        input_identifier: &Arc<str>,
        timestamp: time::OffsetDateTime,
    ) -> Result<Result<(), HashAlreadyExists>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let timestamp = to_primitive(timestamp);

        let res = diesel::insert_into(hashes)
            .values((
                hash.eq(&input_hash),
                identifier.eq(input_identifier.as_ref()),
                created_at.eq(&timestamp),
            ))
            .execute(&mut conn)
            .await;

        match res {
            Ok(_) => Ok(Ok(())),
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Ok(Err(HashAlreadyExists)),
            Err(e) => Err(PostgresError::Diesel(e).into()),
        }
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn update_identifier(
        &self,
        input_hash: Hash,
        input_identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::update(hashes)
            .filter(hash.eq(&input_hash))
            .set(identifier.eq(input_identifier.as_ref()))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn identifier(&self, input_hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = hashes
            .select(identifier)
            .filter(hash.eq(&input_hash))
            .get_result::<String>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt.map(Arc::from))
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn relate_variant_identifier(
        &self,
        input_hash: Hash,
        input_variant: String,
        input_identifier: &Arc<str>,
    ) -> Result<Result<(), VariantAlreadyExists>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.get_connection().await?;

        let res = diesel::insert_into(variants)
            .values((
                hash.eq(&input_hash),
                variant.eq(&input_variant),
                identifier.eq(input_identifier.as_ref()),
            ))
            .execute(&mut conn)
            .await;

        match res {
            Ok(_) => Ok(Ok(())),
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Ok(Err(VariantAlreadyExists)),
            Err(e) => Err(PostgresError::Diesel(e).into()),
        }
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn variant_identifier(
        &self,
        input_hash: Hash,
        input_variant: String,
    ) -> Result<Option<Arc<str>>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = variants
            .select(identifier)
            .filter(hash.eq(&input_hash))
            .filter(variant.eq(&input_variant))
            .get_result::<String>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(Arc::from);

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn variants(&self, input_hash: Hash) -> Result<Vec<(String, Arc<str>)>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.get_connection().await?;

        let vec = variants
            .select((variant, identifier))
            .filter(hash.eq(&input_hash))
            .get_results::<(String, String)>(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?
            .into_iter()
            .map(|(s, i)| (s, Arc::from(i)))
            .collect();

        Ok(vec)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn remove_variant(
        &self,
        input_hash: Hash,
        input_variant: String,
    ) -> Result<(), RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(variants)
            .filter(hash.eq(&input_hash))
            .filter(variant.eq(&input_variant))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn relate_motion_identifier(
        &self,
        input_hash: Hash,
        input_identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::update(hashes)
            .filter(hash.eq(&input_hash))
            .set(motion_identifier.eq(input_identifier.as_ref()))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn motion_identifier(&self, input_hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = hashes
            .select(motion_identifier)
            .filter(hash.eq(&input_hash))
            .get_result::<Option<String>>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?
            .flatten()
            .map(Arc::from);

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn cleanup_hash(&self, input_hash: Hash) -> Result<(), RepoError> {
        let mut conn = self.get_connection().await?;

        conn.transaction(|conn| {
            Box::pin(async move {
                diesel::delete(schema::variants::dsl::variants)
                    .filter(schema::variants::dsl::hash.eq(&input_hash))
                    .execute(conn)
                    .await?;

                diesel::delete(schema::hashes::dsl::hashes)
                    .filter(schema::hashes::dsl::hash.eq(&input_hash))
                    .execute(conn)
                    .await
            })
        })
        .await
        .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl AliasRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn create_alias(
        &self,
        input_alias: &Alias,
        delete_token: &DeleteToken,
        input_hash: Hash,
    ) -> Result<Result<(), AliasAlreadyExists>, RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        let res = diesel::insert_into(aliases)
            .values((
                alias.eq(input_alias),
                hash.eq(&input_hash),
                token.eq(delete_token),
            ))
            .execute(&mut conn)
            .await;

        match res {
            Ok(_) => Ok(Ok(())),
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Ok(Err(AliasAlreadyExists)),
            Err(e) => Err(PostgresError::Diesel(e).into()),
        }
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn delete_token(&self, input_alias: &Alias) -> Result<Option<DeleteToken>, RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = aliases
            .select(token)
            .filter(alias.eq(input_alias))
            .get_result(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn hash(&self, input_alias: &Alias) -> Result<Option<Hash>, RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = aliases
            .select(hash)
            .filter(alias.eq(input_alias))
            .get_result(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn aliases_for_hash(&self, input_hash: Hash) -> Result<Vec<Alias>, RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        let vec = aliases
            .select(alias)
            .filter(hash.eq(&input_hash))
            .get_results(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(vec)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn cleanup_alias(&self, input_alias: &Alias) -> Result<(), RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(aliases)
            .filter(alias.eq(input_alias))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl SettingsRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self, input_value))]
    async fn set(&self, input_key: &'static str, input_value: Arc<[u8]>) -> Result<(), RepoError> {
        use schema::settings::dsl::*;

        let input_value = hex::encode(input_value);

        let mut conn = self.get_connection().await?;

        diesel::insert_into(settings)
            .values((key.eq(input_key), value.eq(&input_value)))
            .on_conflict(key)
            .do_update()
            .set(value.eq(&input_value))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn get(&self, input_key: &'static str) -> Result<Option<Arc<[u8]>>, RepoError> {
        use schema::settings::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = settings
            .select(value)
            .filter(key.eq(input_key))
            .get_result::<String>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(hex::decode)
            .transpose()
            .map_err(PostgresError::Hex)?
            .map(Arc::from);

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn remove(&self, input_key: &'static str) -> Result<(), RepoError> {
        use schema::settings::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(settings)
            .filter(key.eq(input_key))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl DetailsRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self, input_details))]
    async fn relate_details(
        &self,
        input_identifier: &Arc<str>,
        input_details: &Details,
    ) -> Result<(), RepoError> {
        use schema::details::dsl::*;

        let mut conn = self.get_connection().await?;

        let value =
            serde_json::to_value(&input_details.inner).map_err(PostgresError::SerializeDetails)?;

        diesel::insert_into(details)
            .values((identifier.eq(input_identifier.as_ref()), json.eq(&value)))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn details(&self, input_identifier: &Arc<str>) -> Result<Option<Details>, RepoError> {
        use schema::details::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = details
            .select(json)
            .filter(identifier.eq(input_identifier.as_ref()))
            .get_result::<serde_json::Value>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(PostgresError::DeserializeDetails)?
            .map(|inner| Details { inner });

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn cleanup_details(&self, input_identifier: &Arc<str>) -> Result<(), RepoError> {
        use schema::details::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(details)
            .filter(identifier.eq(input_identifier.as_ref()))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl QueueRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self, job_json))]
    async fn push(
        &self,
        queue_name: &'static str,
        job_json: serde_json::Value,
    ) -> Result<JobId, RepoError> {
        use schema::job_queue::dsl::*;

        let mut conn = self.get_connection().await?;

        let job_id = diesel::insert_into(job_queue)
            .values((queue.eq(queue_name), job.eq(job_json)))
            .returning(id)
            .get_result::<Uuid>(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(JobId(job_id))
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn pop(
        &self,
        queue_name: &'static str,
        worker_id: Uuid,
    ) -> Result<(JobId, serde_json::Value), RepoError> {
        use schema::job_queue::dsl::*;

        loop {
            let mut conn = self.get_connection().await?;

            let notifier: Arc<Notify> = self
                .inner
                .queue_notifications
                .entry(String::from(queue_name))
                .or_insert_with(|| Arc::new(Notify::new()))
                .clone();

            diesel::sql_query("LISTEN queue_status_channel;")
                .execute(&mut conn)
                .await
                .map_err(PostgresError::Diesel)?;

            let timestamp = to_primitive(time::OffsetDateTime::now_utc());

            let count = diesel::update(job_queue)
                .filter(heartbeat.le(timestamp.saturating_sub(time::Duration::minutes(2))))
                .set((
                    heartbeat.eq(Option::<time::PrimitiveDateTime>::None),
                    status.eq(JobStatus::New),
                ))
                .execute(&mut conn)
                .await
                .map_err(PostgresError::Diesel)?;

            if count > 0 {
                tracing::info!("Reset {count} jobs");
            }

            // TODO: combine into 1 query
            let opt = loop {
                let id_opt = job_queue
                    .select(id)
                    .filter(status.eq(JobStatus::New).and(queue.eq(queue_name)))
                    .order(queue_time)
                    .limit(1)
                    .get_result::<Uuid>(&mut conn)
                    .await
                    .optional()
                    .map_err(PostgresError::Diesel)?;

                let Some(id_val) = id_opt else {
                    break None;
                };

                let opt = diesel::update(job_queue)
                    .filter(id.eq(id_val))
                    .filter(status.eq(JobStatus::New))
                    .set((
                        heartbeat.eq(timestamp),
                        status.eq(JobStatus::Running),
                        worker.eq(worker_id),
                    ))
                    .returning((id, job))
                    .get_result(&mut conn)
                    .await
                    .optional()
                    .map_err(PostgresError::Diesel)?;

                if let Some(tup) = opt {
                    break Some(tup);
                }
            };

            if let Some((job_id, job_json)) = opt {
                return Ok((JobId(job_id), job_json));
            }

            drop(conn);
            if actix_rt::time::timeout(Duration::from_secs(5), notifier.notified())
                .await
                .is_ok()
            {
                tracing::debug!("Notified");
            } else {
                tracing::debug!("Timed out");
            }
        }
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn heartbeat(
        &self,
        queue_name: &'static str,
        worker_id: Uuid,
        job_id: JobId,
    ) -> Result<(), RepoError> {
        use schema::job_queue::dsl::*;

        let mut conn = self.get_connection().await?;

        let timestamp = to_primitive(time::OffsetDateTime::now_utc());

        diesel::update(job_queue)
            .filter(
                id.eq(job_id.0)
                    .and(queue.eq(queue_name))
                    .and(worker.eq(worker_id)),
            )
            .set(heartbeat.eq(timestamp))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn complete_job(
        &self,
        queue_name: &'static str,
        worker_id: Uuid,
        job_id: JobId,
    ) -> Result<(), RepoError> {
        use schema::job_queue::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(job_queue)
            .filter(
                id.eq(job_id.0)
                    .and(queue.eq(queue_name))
                    .and(worker.eq(worker_id)),
            )
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl StoreMigrationRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn is_continuing_migration(&self) -> Result<bool, RepoError> {
        use schema::store_migrations::dsl::*;

        let mut conn = self.get_connection().await?;

        let count = store_migrations
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(count > 0)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn mark_migrated(
        &self,
        input_old_identifier: &Arc<str>,
        input_new_identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        use schema::store_migrations::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::insert_into(store_migrations)
            .values((
                old_identifier.eq(input_old_identifier.as_ref()),
                new_identifier.eq(input_new_identifier.as_ref()),
            ))
            .on_conflict((old_identifier, new_identifier))
            .do_nothing()
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn is_migrated(&self, input_old_identifier: &Arc<str>) -> Result<bool, RepoError> {
        use schema::store_migrations::dsl::*;

        let mut conn = self.get_connection().await?;

        let b = diesel::select(diesel::dsl::exists(
            store_migrations.filter(old_identifier.eq(input_old_identifier.as_ref())),
        ))
        .get_result(&mut conn)
        .await
        .map_err(PostgresError::Diesel)?;

        Ok(b)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn clear(&self) -> Result<(), RepoError> {
        use schema::store_migrations::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(store_migrations)
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ProxyRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn relate_url(&self, input_url: Url, input_alias: Alias) -> Result<(), RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::insert_into(proxies)
            .values((url.eq(input_url.as_str()), alias.eq(&input_alias)))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn related(&self, input_url: Url) -> Result<Option<Alias>, RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = proxies
            .select(alias)
            .filter(url.eq(input_url.as_str()))
            .get_result(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn remove_relation(&self, input_alias: Alias) -> Result<(), RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(proxies)
            .filter(alias.eq(&input_alias))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl AliasAccessRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn set_accessed_alias(
        &self,
        input_alias: Alias,
        timestamp: time::OffsetDateTime,
    ) -> Result<(), RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        let timestamp = to_primitive(timestamp);

        diesel::update(proxies)
            .filter(alias.eq(&input_alias))
            .set(accessed.eq(timestamp))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn alias_accessed_at(
        &self,
        input_alias: Alias,
    ) -> Result<Option<time::OffsetDateTime>, RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = proxies
            .select(accessed)
            .filter(alias.eq(&input_alias))
            .get_result::<time::PrimitiveDateTime>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(time::PrimitiveDateTime::assume_utc);

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn older_aliases(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<Alias, RepoError>>, RepoError> {
        Ok(Box::pin(PageStream {
            inner: self.inner.clone(),
            future: None,
            current: Vec::new(),
            older_than: to_primitive(timestamp),
            next: Box::new(|inner, older_than| {
                Box::pin(async move {
                    use schema::proxies::dsl::*;

                    let mut conn = inner.get_connection().await?;

                    let vec = proxies
                        .select((accessed, alias))
                        .filter(accessed.lt(older_than))
                        .order(accessed.desc())
                        .limit(100)
                        .get_results(&mut conn)
                        .await
                        .map_err(PostgresError::Diesel)?;

                    Ok(vec)
                })
            }),
        }))
    }

    async fn remove_alias_access(&self, _: Alias) -> Result<(), RepoError> {
        // Noop - handled by ProxyRepo::remove_relation
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl VariantAccessRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn set_accessed_variant(
        &self,
        input_hash: Hash,
        input_variant: String,
        input_accessed: time::OffsetDateTime,
    ) -> Result<(), RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.get_connection().await?;

        let timestamp = to_primitive(input_accessed);

        diesel::update(variants)
            .filter(hash.eq(&input_hash).and(variant.eq(&input_variant)))
            .set(accessed.eq(timestamp))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn variant_accessed_at(
        &self,
        input_hash: Hash,
        input_variant: String,
    ) -> Result<Option<time::OffsetDateTime>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = variants
            .select(accessed)
            .filter(hash.eq(&input_hash).and(variant.eq(&input_variant)))
            .get_result(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(time::PrimitiveDateTime::assume_utc);

        Ok(opt)
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn older_variants(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<(Hash, String), RepoError>>, RepoError> {
        Ok(Box::pin(PageStream {
            inner: self.inner.clone(),
            future: None,
            current: Vec::new(),
            older_than: to_primitive(timestamp),
            next: Box::new(|inner, older_than| {
                Box::pin(async move {
                    use schema::variants::dsl::*;

                    let mut conn = inner.get_connection().await?;

                    let vec = variants
                        .select((accessed, (hash, variant)))
                        .filter(accessed.lt(older_than))
                        .order(accessed.desc())
                        .limit(100)
                        .get_results(&mut conn)
                        .await
                        .map_err(PostgresError::Diesel)?;

                    Ok(vec)
                })
            }),
        }))
    }

    async fn remove_variant_access(&self, _: Hash, _: String) -> Result<(), RepoError> {
        // Noop - handled by HashRepo::remove_variant
        Ok(())
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
enum InnerUploadResult {
    Success {
        alias: Serde<Alias>,
        token: Serde<DeleteToken>,
    },
    Failure {
        message: String,
        code: OwnedErrorCode,
    },
}

impl From<UploadResult> for InnerUploadResult {
    fn from(u: UploadResult) -> Self {
        match u {
            UploadResult::Success { alias, token } => InnerUploadResult::Success {
                alias: Serde::new(alias),
                token: Serde::new(token),
            },
            UploadResult::Failure { message, code } => InnerUploadResult::Failure { message, code },
        }
    }
}

impl From<InnerUploadResult> for UploadResult {
    fn from(i: InnerUploadResult) -> Self {
        match i {
            InnerUploadResult::Success { alias, token } => UploadResult::Success {
                alias: Serde::into_inner(alias),
                token: Serde::into_inner(token),
            },
            InnerUploadResult::Failure { message, code } => UploadResult::Failure { message, code },
        }
    }
}

#[async_trait::async_trait(?Send)]
impl UploadRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn create_upload(&self) -> Result<UploadId, RepoError> {
        use schema::uploads::dsl::*;

        let mut conn = self.get_connection().await?;

        let uuid = diesel::insert_into(uploads)
            .default_values()
            .returning(id)
            .get_result(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(UploadId { id: uuid })
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, RepoError> {
        use schema::uploads::dsl::*;

        loop {
            let mut conn = self.get_connection().await?;

            diesel::sql_query("LISTEN upload_completion_channel;")
                .execute(&mut conn)
                .await
                .map_err(PostgresError::Diesel)?;

            let opt = uploads
                .select(result)
                .filter(id.eq(upload_id.id))
                .get_result(&mut conn)
                .await
                .optional()
                .map_err(PostgresError::Diesel)?
                .flatten();

            if let Some(upload_result) = opt {
                let upload_result: InnerUploadResult = serde_json::from_value(upload_result)
                    .map_err(PostgresError::DeserializeUploadResult)?;

                return Ok(upload_result.into());
            }

            drop(conn);

            if actix_rt::time::timeout(
                Duration::from_secs(5),
                self.inner.upload_notifier.notified(),
            )
            .await
            .is_ok()
            {
                tracing::debug!("Notified");
            } else {
                tracing::debug!("Timed out");
            }
        }
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn claim(&self, upload_id: UploadId) -> Result<(), RepoError> {
        use schema::uploads::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(uploads)
            .filter(id.eq(upload_id.id))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn complete_upload(
        &self,
        upload_id: UploadId,
        upload_result: UploadResult,
    ) -> Result<(), RepoError> {
        use schema::uploads::dsl::*;

        let mut conn = self.get_connection().await?;

        let upload_result: InnerUploadResult = upload_result.into();
        let upload_result =
            serde_json::to_value(&upload_result).map_err(PostgresError::SerializeUploadResult)?;

        diesel::update(uploads)
            .filter(id.eq(upload_id.id))
            .set(result.eq(upload_result))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl FullRepo for PostgresRepo {
    #[tracing::instrument(level = "DEBUG", skip(self))]
    async fn health_check(&self) -> Result<(), RepoError> {
        let next = self.inner.health_count.fetch_add(1, Ordering::Relaxed);

        self.set("health-value", Arc::from(next.to_be_bytes()))
            .await?;

        Ok(())
    }
}

type LocalBoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

type NextFuture<I> = LocalBoxFuture<'static, Result<Vec<(time::PrimitiveDateTime, I)>, RepoError>>;

struct PageStream<I> {
    inner: Arc<Inner>,
    future: Option<NextFuture<I>>,
    current: Vec<I>,
    older_than: time::PrimitiveDateTime,
    next: Box<dyn Fn(Arc<Inner>, time::PrimitiveDateTime) -> NextFuture<I>>,
}

impl<I> futures_core::Stream for PageStream<I>
where
    I: Unpin,
{
    type Item = Result<I, RepoError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // Pop because we reversed the list
            if let Some(alias) = this.current.pop() {
                return std::task::Poll::Ready(Some(Ok(alias)));
            }

            if let Some(future) = this.future.as_mut() {
                let res = std::task::ready!(future.as_mut().poll(cx));

                this.future.take();

                match res {
                    Ok(page) if page.is_empty() => {
                        return std::task::Poll::Ready(None);
                    }
                    Ok(page) => {
                        let (mut timestamps, mut aliases): (Vec<_>, Vec<_>) =
                            page.into_iter().unzip();
                        // reverse because we use .pop() to get next
                        aliases.reverse();

                        this.current = aliases;
                        this.older_than = timestamps.pop().expect("Verified nonempty");
                    }
                    Err(e) => return std::task::Poll::Ready(Some(Err(e))),
                }
            } else {
                let inner = this.inner.clone();
                let older_than = this.older_than;

                this.future = Some((this.next)(inner, older_than));
            }
        }
    }
}

impl std::fmt::Debug for PostgresRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRepo")
            .field("pool", &"pool")
            .finish()
    }
}
