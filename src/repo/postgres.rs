mod embedded;
mod job_status;
mod schema;

use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
    time::{Duration, Instant},
};

use dashmap::DashMap;
use diesel::prelude::*;
use diesel_async::{
    pooled_connection::{
        deadpool::{BuildError, Hook, Object, Pool, PoolError},
        AsyncDieselConnectionManager, ManagerConfig,
    },
    AsyncConnection, AsyncPgConnection, RunQueryDsl,
};
use futures_core::Stream;
use tokio::sync::Notify;
use tokio_postgres::{AsyncMessage, Connection, NoTls, Notification, Socket};
use tokio_postgres_rustls::MakeRustlsConnect;
use tracing::Instrument;
use url::Url;
use uuid::Uuid;

use crate::{
    details::Details,
    error_code::{ErrorCode, OwnedErrorCode},
    future::{WithMetrics, WithTimeout},
    serde_str::Serde,
    stream::LocalBoxStream,
    sync::DropHandle,
};

use self::job_status::JobStatus;

use super::{
    metrics::{PopMetricsGuard, PushMetricsGuard, WaitMetricsGuard},
    Alias, AliasAccessRepo, AliasAlreadyExists, AliasRepo, BaseRepo, DeleteToken, DetailsRepo,
    FullRepo, Hash, HashAlreadyExists, HashPage, HashRepo, JobId, OrderedHash, ProxyRepo,
    QueueRepo, RepoError, SettingsRepo, StoreMigrationRepo, UploadId, UploadRepo, UploadResult,
    VariantAccessRepo, VariantAlreadyExists,
};

#[derive(Clone)]
pub(crate) struct PostgresRepo {
    inner: Arc<Inner>,
    #[allow(dead_code)]
    notifications: Arc<DropHandle<()>>,
}

struct Inner {
    health_count: AtomicU64,
    pool: Pool<AsyncPgConnection>,
    notifier_pool: Pool<AsyncPgConnection>,
    queue_notifications: DashMap<String, Arc<Notify>>,
    upload_notifications: DashMap<UploadId, Weak<Notify>>,
}

struct UploadInterest {
    inner: Arc<Inner>,
    interest: Option<Arc<Notify>>,
    upload_id: UploadId,
}

struct JobNotifierState<'a> {
    inner: &'a Inner,
    capacity: usize,
    jobs: BTreeSet<JobId>,
    jobs_ordered: VecDeque<JobId>,
}

struct UploadNotifierState<'a> {
    inner: &'a Inner,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectPostgresError {
    #[error("Failed to connect to postgres for migrations")]
    ConnectForMigration(#[source] tokio_postgres::Error),

    #[error("Failed to build TLS configuration")]
    Tls(#[source] TlsError),

    #[error("Failed to run migrations")]
    Migration(#[source] Box<refinery::Error>),

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

    #[error("Timed out waiting for postgres")]
    DbTimeout,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TlsError {
    #[error("Couldn't read configured certificate file")]
    Read(#[source] std::io::Error),

    #[error("Couldn't parse configured certificate file: {0:?}")]
    Parse(rustls_pemfile::Error),

    #[error("Configured certificate file is not a certificate")]
    Invalid,

    #[error("Couldn't add certificate to root store")]
    Add(#[source] rustls::Error),
}

impl PostgresError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Pool(_)
            | Self::Diesel(_)
            | Self::SerializeDetails(_)
            | Self::SerializeUploadResult(_)
            | Self::Hex(_)
            | Self::DbTimeout => ErrorCode::POSTGRES_ERROR,
            Self::DeserializeDetails(_) => ErrorCode::EXTRACT_DETAILS,
            Self::DeserializeUploadResult(_) => ErrorCode::EXTRACT_UPLOAD_RESULT,
        }
    }

    pub(super) const fn is_disconnected(&self) -> bool {
        matches!(
            self,
            Self::Pool(
                PoolError::Closed
                    | PoolError::Backend(
                        diesel_async::pooled_connection::PoolError::ConnectionError(_)
                    ),
            ) | Self::Diesel(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::ClosedConnection,
                _,
            ))
        )
    }
}

async fn build_tls_connector(
    certificate_file: Option<PathBuf>,
) -> Result<MakeRustlsConnect, TlsError> {
    let mut cert_store = rustls::RootCertStore {
        roots: Vec::from(webpki_roots::TLS_SERVER_ROOTS),
    };

    if let Some(certificate_file) = certificate_file {
        let bytes = tokio::fs::read(certificate_file)
            .await
            .map_err(TlsError::Read)?;

        let opt = rustls_pemfile::read_one_from_slice(&bytes).map_err(TlsError::Parse)?;
        let (item, _remainder) = opt.ok_or(TlsError::Invalid)?;

        let cert = if let rustls_pemfile::Item::X509Certificate(cert) = item {
            cert
        } else {
            return Err(TlsError::Invalid);
        };

        cert_store.add(cert).map_err(TlsError::Add)?;
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(cert_store)
        .with_no_client_auth();

    let tls = MakeRustlsConnect::new(config);

    Ok(tls)
}

async fn connect_for_migrations(
    postgres_url: &Url,
    tls_connector: Option<MakeRustlsConnect>,
) -> Result<
    (
        tokio_postgres::Client,
        DropHandle<Result<(), tokio_postgres::Error>>,
    ),
    ConnectPostgresError,
> {
    let tup = if let Some(connector) = tls_connector {
        let (client, conn) = tokio_postgres::connect(postgres_url.as_str(), connector)
            .await
            .map_err(ConnectPostgresError::ConnectForMigration)?;

        (
            client,
            crate::sync::abort_on_drop(crate::sync::spawn("postgres-connection", conn)),
        )
    } else {
        let (client, conn) = tokio_postgres::connect(postgres_url.as_str(), NoTls)
            .await
            .map_err(ConnectPostgresError::ConnectForMigration)?;

        (
            client,
            crate::sync::abort_on_drop(crate::sync::spawn("postgres-connection", conn)),
        )
    };

    Ok(tup)
}

fn build_pool(
    postgres_url: &Url,
    tx: flume::Sender<Notification>,
    connector: Option<MakeRustlsConnect>,
    max_size: usize,
) -> Result<Pool<AsyncPgConnection>, ConnectPostgresError> {
    let mut config = ManagerConfig::default();
    config.custom_setup = build_handler(tx, connector);

    let mgr = AsyncDieselConnectionManager::<AsyncPgConnection>::new_with_config(
        postgres_url.to_string(),
        config,
    );

    let pool = Pool::builder(mgr)
        .runtime(deadpool::Runtime::Tokio1)
        .wait_timeout(Some(Duration::from_secs(10)))
        .create_timeout(Some(Duration::from_secs(2)))
        .recycle_timeout(Some(Duration::from_secs(2)))
        .post_create(Hook::sync_fn(|_, _| {
            metrics::counter!("pict-rs.postgres.pool.connection.create").increment(1);
            Ok(())
        }))
        .post_recycle(Hook::sync_fn(|_, _| {
            metrics::counter!("pict-rs.postgres.pool.connection.recycle").increment(1);
            Ok(())
        }))
        .max_size(max_size)
        .build()
        .map_err(ConnectPostgresError::BuildPool)?;

    Ok(pool)
}

impl PostgresRepo {
    pub(crate) async fn connect(
        postgres_url: Url,
        use_tls: bool,
        certificate_file: Option<PathBuf>,
    ) -> Result<Self, ConnectPostgresError> {
        let connector = if use_tls {
            Some(
                build_tls_connector(certificate_file)
                    .await
                    .map_err(ConnectPostgresError::Tls)?,
            )
        } else {
            None
        };

        let (mut client, handle) = connect_for_migrations(&postgres_url, connector.clone()).await?;

        embedded::migrations::runner()
            .run_async(&mut client)
            .await
            .map_err(Box::new)
            .map_err(ConnectPostgresError::Migration)?;

        handle.abort();
        let _ = handle.await;

        let parallelism = std::thread::available_parallelism()
            .map(|u| u.into())
            .unwrap_or(1_usize);

        let (tx, rx) = flume::bounded(10);

        let inner = Arc::new(Inner {
            health_count: AtomicU64::new(0),
            pool: build_pool(
                &postgres_url,
                tx.clone(),
                connector.clone(),
                parallelism * 8,
            )?,
            notifier_pool: build_pool(&postgres_url, tx, connector, parallelism.min(4))?,
            queue_notifications: DashMap::new(),
            upload_notifications: DashMap::new(),
        });

        let handle = crate::sync::abort_on_drop(crate::sync::spawn(
            "postgres-delegate-notifications",
            delegate_notifications(rx, inner.clone(), parallelism * 8),
        ));

        let notifications = Arc::new(handle);

        Ok(PostgresRepo {
            inner,
            notifications,
        })
    }

    async fn get_connection(&self) -> Result<Object<AsyncPgConnection>, PostgresError> {
        self.inner.get_connection().await
    }

    async fn get_notifier_connection(&self) -> Result<Object<AsyncPgConnection>, PostgresError> {
        self.inner.get_notifier_connection().await
    }
}

struct GetConnectionMetricsGuard {
    start: Instant,
    armed: bool,
}

impl GetConnectionMetricsGuard {
    fn guard() -> Self {
        GetConnectionMetricsGuard {
            start: Instant::now(),
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for GetConnectionMetricsGuard {
    fn drop(&mut self) {
        metrics::counter!("pict-rs.postgres.pool.get", "completed" => (!self.armed).to_string())
            .increment(1);
        metrics::histogram!("pict-rs.postgres.pool.get.duration", "completed" => (!self.armed).to_string()).record(self.start.elapsed().as_secs_f64());
    }
}

impl Inner {
    #[tracing::instrument(level = "trace", skip(self))]
    async fn get_connection(&self) -> Result<Object<AsyncPgConnection>, PostgresError> {
        let guard = GetConnectionMetricsGuard::guard();

        let obj = self.pool.get().await.map_err(PostgresError::Pool)?;

        guard.disarm();

        Ok(obj)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    async fn get_notifier_connection(&self) -> Result<Object<AsyncPgConnection>, PostgresError> {
        let guard = GetConnectionMetricsGuard::guard();

        let obj = self
            .notifier_pool
            .get()
            .await
            .map_err(PostgresError::Pool)?;

        guard.disarm();

        Ok(obj)
    }

    fn interest(self: &Arc<Self>, upload_id: UploadId) -> UploadInterest {
        let notify = crate::sync::notify();

        self.upload_notifications
            .insert(upload_id, Arc::downgrade(&notify));

        UploadInterest {
            inner: self.clone(),
            interest: Some(notify),
            upload_id,
        }
    }
}

impl UploadInterest {
    async fn notified_timeout(&self, timeout: Duration) -> Result<(), tokio::time::error::Elapsed> {
        self.interest
            .as_ref()
            .expect("interest exists")
            .notified()
            .with_timeout(timeout)
            .await
    }
}

impl Drop for UploadInterest {
    fn drop(&mut self) {
        if let Some(interest) = self.interest.take() {
            if Arc::into_inner(interest).is_some() {
                self.inner.upload_notifications.remove(&self.upload_id);
            }
        }
    }
}

impl<'a> JobNotifierState<'a> {
    fn handle(&mut self, payload: &str) {
        let Some((job_id, queue_name)) = payload.split_once(' ') else {
            tracing::warn!("Invalid queue payload {payload}");
            return;
        };

        let Ok(job_id) = job_id.parse::<Uuid>().map(JobId) else {
            tracing::warn!("Invalid job ID {job_id}");
            return;
        };

        if !self.jobs.insert(job_id) {
            // duplicate job
            return;
        }

        self.jobs_ordered.push_back(job_id);

        if self.jobs_ordered.len() > self.capacity {
            if let Some(job_id) = self.jobs_ordered.pop_front() {
                self.jobs.remove(&job_id);
            }
        }

        self.inner
            .queue_notifications
            .entry(queue_name.to_string())
            .or_insert_with(crate::sync::notify)
            .notify_one();

        metrics::counter!("pict-rs.postgres.job-notifier.notified", "queue" => queue_name.to_string()).increment(1);
    }
}

impl<'a> UploadNotifierState<'a> {
    fn handle(&self, payload: &str) {
        let Ok(upload_id) = payload.parse::<UploadId>() else {
            tracing::warn!("Invalid upload id {payload}");
            return;
        };

        if let Some(notifier) = self
            .inner
            .upload_notifications
            .get(&upload_id)
            .and_then(|weak| weak.upgrade())
        {
            notifier.notify_waiters();
            metrics::counter!("pict-rs.postgres.upload-notifier.notified").increment(1);
        }
    }
}

type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
type ConfigFn =
    Box<dyn Fn(&str) -> BoxFuture<'_, ConnectionResult<AsyncPgConnection>> + Send + Sync + 'static>;

async fn delegate_notifications(
    receiver: flume::Receiver<Notification>,
    inner: Arc<Inner>,
    capacity: usize,
) {
    let mut job_notifier_state = JobNotifierState {
        inner: &inner,
        capacity,
        jobs: BTreeSet::new(),
        jobs_ordered: VecDeque::new(),
    };

    let upload_notifier_state = UploadNotifierState { inner: &inner };

    while let Ok(notification) = receiver.recv_async().await {
        tracing::trace!("delegate_notifications: looping");
        metrics::counter!("pict-rs.postgres.notification").increment(1);

        match notification.channel() {
            "queue_status_channel" => {
                // new job inserted for queue
                job_notifier_state.handle(notification.payload());
            }
            "upload_completion_channel" => {
                // new upload finished
                upload_notifier_state.handle(notification.payload());
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

fn build_handler(
    sender: flume::Sender<Notification>,
    connector: Option<MakeRustlsConnect>,
) -> ConfigFn {
    Box::new(
        move |config: &str| -> BoxFuture<'_, ConnectionResult<AsyncPgConnection>> {
            let sender = sender.clone();
            let connector = connector.clone();

            let connect_span = tracing::trace_span!(parent: None, "connect future");

            Box::pin(
                async move {
                    let client = if let Some(connector) = connector {
                        let (client, conn) = tokio_postgres::connect(config, connector)
                            .await
                            .map_err(|e| ConnectionError::BadConnection(e.to_string()))?;

                        // not very cash money (structured concurrency) of me
                        spawn_db_notification_task(sender, conn);

                        client
                    } else {
                        let (client, conn) =
                            tokio_postgres::connect(config, tokio_postgres::tls::NoTls)
                                .await
                                .map_err(|e| ConnectionError::BadConnection(e.to_string()))?;

                        // not very cash money (structured concurrency) of me
                        spawn_db_notification_task(sender, conn);

                        client
                    };

                    AsyncPgConnection::try_from(client).await
                }
                .instrument(connect_span),
            )
        },
    )
}

fn spawn_db_notification_task<S>(
    sender: flume::Sender<Notification>,
    mut conn: Connection<Socket, S>,
) where
    S: tokio_postgres::tls::TlsStream + Unpin + 'static,
{
    crate::sync::spawn("postgres-notifications", async move {
        while let Some(res) = std::future::poll_fn(|cx| conn.poll_message(cx)).await {
            tracing::trace!("db_notification_task: looping");

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
}

fn to_primitive(timestamp: time::OffsetDateTime) -> time::PrimitiveDateTime {
    let timestamp = timestamp.to_offset(time::UtcOffset::UTC);
    time::PrimitiveDateTime::new(timestamp.date(), timestamp.time())
}

impl BaseRepo for PostgresRepo {}

#[async_trait::async_trait(?Send)]
impl HashRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self))]
    async fn size(&self) -> Result<u64, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let count = hashes
            .count()
            .get_result::<i64>(&mut conn)
            .with_metrics("pict-rs.postgres.hashes.count")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(count.try_into().expect("non-negative count"))
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn bound(&self, input_hash: Hash) -> Result<Option<OrderedHash>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let timestamp = hashes
            .select(created_at)
            .filter(hash.eq(&input_hash))
            .get_result(&mut conn)
            .with_metrics("pict-rs.postgres.hashes.bound")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map(time::PrimitiveDateTime::assume_utc)
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(timestamp.map(|timestamp| OrderedHash {
            timestamp,
            hash: input_hash,
        }))
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .get_result::<(time::PrimitiveDateTime, Hash)>(&mut conn)
            .with_metrics("pict-rs.postgres.hashes.ordered-hash")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(|tup| OrderedHash {
                timestamp: tup.0.assume_utc(),
                hash: tup.1,
            });

        self.hashes_ordered(ordered_hash, limit).await
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
                .get_results::<Hash>(&mut conn)
                .with_metrics("pict-rs.postgres.hashes.next-hashes")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
                .map_err(PostgresError::Diesel)?;

            let prev = hashes
                .select(hash)
                .filter(created_at.gt(timestamp))
                .or_filter(created_at.eq(timestamp).and(hash.gt(&bound_hash)))
                .order(created_at)
                .then_order_by(hash)
                .limit(limit as i64)
                .get_results::<Hash>(&mut conn)
                .with_metrics("pict-rs.postgres.hashes.prev-hashes")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
                .map_err(PostgresError::Diesel)?
                .pop();

            (page, prev)
        } else {
            let page = hashes
                .select(hash)
                .order(created_at.desc())
                .then_order_by(hash.desc())
                .limit(limit as i64 + 1)
                .get_results::<Hash>(&mut conn)
                .with_metrics("pict-rs.postgres.hashes.first-hashes")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
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

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.hashes.create-hash")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?;

        match res {
            Ok(_) => Ok(Ok(())),
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Ok(Err(HashAlreadyExists)),
            Err(e) => Err(PostgresError::Diesel(e).into()),
        }
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.hashes.update-identifier")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn identifier(&self, input_hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = hashes
            .select(identifier)
            .filter(hash.eq(&input_hash))
            .get_result::<String>(&mut conn)
            .with_metrics("pict-rs.postgres.hashes.identifier")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt.map(Arc::from))
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.variants.relate-variant-identifier")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?;

        match res {
            Ok(_) => Ok(Ok(())),
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Ok(Err(VariantAlreadyExists)),
            Err(e) => Err(PostgresError::Diesel(e).into()),
        }
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.variants.identifier")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(Arc::from);

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn variants(&self, input_hash: Hash) -> Result<Vec<(String, Arc<str>)>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.get_connection().await?;

        let vec = variants
            .select((variant, identifier))
            .filter(hash.eq(&input_hash))
            .get_results::<(String, String)>(&mut conn)
            .with_metrics("pict-rs.postgres.variants.for-hash")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?
            .into_iter()
            .map(|(s, i)| (s, Arc::from(i)))
            .collect();

        Ok(vec)
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.variants.remove")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.hashes.relate-motion-identifier")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn motion_identifier(&self, input_hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = hashes
            .select(motion_identifier)
            .filter(hash.eq(&input_hash))
            .get_result::<Option<String>>(&mut conn)
            .with_metrics("pict-rs.postgres.hashes.motion-identifier")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?
            .flatten()
            .map(Arc::from);

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn cleanup_hash(&self, input_hash: Hash) -> Result<(), RepoError> {
        let mut conn = self.get_connection().await?;

        conn.transaction(|conn| {
            Box::pin(async move {
                diesel::delete(schema::variants::dsl::variants)
                    .filter(schema::variants::dsl::hash.eq(&input_hash))
                    .execute(conn)
                    .with_metrics("pict-rs.postgres.variants.cleanup")
                    .await?;

                diesel::delete(schema::hashes::dsl::hashes)
                    .filter(schema::hashes::dsl::hash.eq(&input_hash))
                    .execute(conn)
                    .with_metrics("pict-rs.postgres.hashes.cleanup")
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
    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.aliases.create")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?;

        match res {
            Ok(_) => Ok(Ok(())),
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Ok(Err(AliasAlreadyExists)),
            Err(e) => Err(PostgresError::Diesel(e).into()),
        }
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn delete_token(&self, input_alias: &Alias) -> Result<Option<DeleteToken>, RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = aliases
            .select(token)
            .filter(alias.eq(input_alias))
            .get_result(&mut conn)
            .with_metrics("pict-rs.postgres.aliases.delete-token")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn hash(&self, input_alias: &Alias) -> Result<Option<Hash>, RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = aliases
            .select(hash)
            .filter(alias.eq(input_alias))
            .get_result(&mut conn)
            .with_metrics("pict-rs.postgres.aliases.hash")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn aliases_for_hash(&self, input_hash: Hash) -> Result<Vec<Alias>, RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        let vec = aliases
            .select(alias)
            .filter(hash.eq(&input_hash))
            .get_results(&mut conn)
            .with_metrics("pict-rs.postgres.aliases.for-hash")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(vec)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn cleanup_alias(&self, input_alias: &Alias) -> Result<(), RepoError> {
        use schema::aliases::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(aliases)
            .filter(alias.eq(input_alias))
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.aliases.cleanup")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl SettingsRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self, input_value))]
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
            .with_metrics("pict-rs.postgres.settings.set")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn get(&self, input_key: &'static str) -> Result<Option<Arc<[u8]>>, RepoError> {
        use schema::settings::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = settings
            .select(value)
            .filter(key.eq(input_key))
            .get_result::<String>(&mut conn)
            .with_metrics("pict-rs.postgres.settings.get")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(hex::decode)
            .transpose()
            .map_err(PostgresError::Hex)?
            .map(Arc::from);

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn remove(&self, input_key: &'static str) -> Result<(), RepoError> {
        use schema::settings::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(settings)
            .filter(key.eq(input_key))
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.settings.remove")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl DetailsRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self, input_details))]
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
            .with_metrics("pict-rs.postgres.details.relate")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn details(&self, input_identifier: &Arc<str>) -> Result<Option<Details>, RepoError> {
        use schema::details::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = details
            .select(json)
            .filter(identifier.eq(input_identifier.as_ref()))
            .get_result::<serde_json::Value>(&mut conn)
            .with_metrics("pict-rs.postgres.details.get")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(PostgresError::DeserializeDetails)?
            .map(|inner| Details { inner });

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn cleanup_details(&self, input_identifier: &Arc<str>) -> Result<(), RepoError> {
        use schema::details::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(details)
            .filter(identifier.eq(input_identifier.as_ref()))
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.details.cleanup")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl QueueRepo for PostgresRepo {
    async fn queue_length(&self) -> Result<u64, RepoError> {
        use schema::job_queue::dsl::*;

        let mut conn = self.get_connection().await?;

        let count = job_queue
            .count()
            .get_result::<i64>(&mut conn)
            .with_metrics("pict-rs.postgres.job_queue.count")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(count.try_into().expect("non-negative count"))
    }

    #[tracing::instrument(level = "debug", skip(self, job_json))]
    async fn push(
        &self,
        queue_name: &'static str,
        job_json: serde_json::Value,
        in_unique_key: Option<&'static str>,
    ) -> Result<Option<JobId>, RepoError> {
        let guard = PushMetricsGuard::guard(queue_name);

        use schema::job_queue::dsl::*;

        let mut conn = self.get_connection().await?;

        let res = diesel::insert_into(job_queue)
            .values((
                queue.eq(queue_name),
                job.eq(job_json),
                unique_key.eq(in_unique_key),
            ))
            .returning(id)
            .get_result::<Uuid>(&mut conn)
            .with_metrics("pict-rs.postgres.queue.push")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map(JobId)
            .map(Some);

        match res {
            Ok(job_id) => {
                guard.disarm();

                Ok(job_id)
            }
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            )) => Ok(None),
            Err(e) => Err(RepoError::from(PostgresError::Diesel(e))),
        }
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn pop(
        &self,
        queue_name: &'static str,
        worker_id: Uuid,
    ) -> Result<(JobId, serde_json::Value), RepoError> {
        let guard = PopMetricsGuard::guard(queue_name);

        use schema::job_queue::dsl::*;

        loop {
            tracing::trace!("pop: looping");

            let notifier: Arc<Notify> = self
                .inner
                .queue_notifications
                .entry(String::from(queue_name))
                .or_insert_with(crate::sync::notify)
                .clone();

            let mut notifier_conn = self.get_notifier_connection().await?;

            diesel::sql_query("LISTEN queue_status_channel;")
                .execute(&mut notifier_conn)
                .with_metrics("pict-rs.postgres.queue.listen")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
                .map_err(PostgresError::Diesel)?;

            drop(notifier_conn);

            let mut conn = self.get_connection().await?;

            let timestamp = to_primitive(time::OffsetDateTime::now_utc());

            let count = diesel::update(job_queue)
                .filter(heartbeat.le(timestamp.saturating_sub(time::Duration::minutes(2))))
                .set((
                    heartbeat.eq(Option::<time::PrimitiveDateTime>::None),
                    status.eq(JobStatus::New),
                    worker.eq(Option::<Uuid>::None),
                ))
                .execute(&mut conn)
                .with_metrics("pict-rs.postgres.queue.requeue")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
                .map_err(PostgresError::Diesel)?;

            if count > 0 {
                tracing::info!("Reset {count} jobs");
            }

            let queue_alias = diesel::alias!(schema::job_queue as queue_alias);

            let id_query = queue_alias
                .select(queue_alias.field(id))
                .filter(
                    queue_alias
                        .field(status)
                        .eq(JobStatus::New)
                        .and(queue_alias.field(queue).eq(queue_name)),
                )
                .order(queue_alias.field(queue_time))
                .for_update()
                .skip_locked()
                .single_value();

            let opt = diesel::update(job_queue)
                .filter(id.nullable().eq(id_query))
                .filter(status.eq(JobStatus::New))
                .set((
                    heartbeat.eq(timestamp),
                    status.eq(JobStatus::Running),
                    worker.eq(worker_id),
                ))
                .returning((id, job))
                .get_result(&mut conn)
                .with_metrics("pict-rs.postgres.queue.claim")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
                .optional()
                .map_err(PostgresError::Diesel)?;

            if let Some((job_id, job_json)) = opt {
                guard.disarm();
                return Ok((JobId(job_id), job_json));
            }

            drop(conn);
            if notifier
                .notified()
                .with_timeout(Duration::from_secs(5))
                .await
                .is_ok()
            {
                tracing::debug!("Notified");
            } else {
                tracing::debug!("Timed out");
            }
        }
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.queue.heartbeat")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.queue.complete")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl StoreMigrationRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self))]
    async fn is_continuing_migration(&self) -> Result<bool, RepoError> {
        use schema::store_migrations::dsl::*;

        let mut conn = self.get_connection().await?;

        let count = store_migrations
            .count()
            .get_result::<i64>(&mut conn)
            .with_metrics("pict-rs.postgres.store-migration.count")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(count > 0)
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .on_conflict(old_identifier)
            .do_nothing()
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.store-migration.mark-migrated")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn is_migrated(&self, input_old_identifier: &Arc<str>) -> Result<bool, RepoError> {
        use schema::store_migrations::dsl::*;

        let mut conn = self.get_connection().await?;

        let b = diesel::select(diesel::dsl::exists(
            store_migrations.filter(old_identifier.eq(input_old_identifier.as_ref())),
        ))
        .get_result(&mut conn)
        .with_metrics("pict-rs.postgres.store-migration.is-migrated")
        .with_timeout(Duration::from_secs(5))
        .await
        .map_err(|_| PostgresError::DbTimeout)?
        .map_err(PostgresError::Diesel)?;

        Ok(b)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn clear(&self) -> Result<(), RepoError> {
        use schema::store_migrations::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(store_migrations)
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.store-migration.clear")
            .with_timeout(Duration::from_secs(20))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl ProxyRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self))]
    async fn relate_url(&self, input_url: Url, input_alias: Alias) -> Result<(), RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::insert_into(proxies)
            .values((url.eq(input_url.as_str()), alias.eq(&input_alias)))
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.proxy.relate-url")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn related(&self, input_url: Url) -> Result<Option<Alias>, RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        let opt = proxies
            .select(alias)
            .filter(url.eq(input_url.as_str()))
            .get_result(&mut conn)
            .with_metrics("pict-rs.postgres.proxy.related")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn remove_relation(&self, input_alias: Alias) -> Result<(), RepoError> {
        use schema::proxies::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(proxies)
            .filter(alias.eq(&input_alias))
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.proxy.remove-relation")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl AliasAccessRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.alias-access.set-accessed")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.alias-access.accessed-at")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(time::PrimitiveDateTime::assume_utc);

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn older_aliases(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<Alias, RepoError>>, RepoError> {
        Ok(Box::pin(page_stream(
            self.inner.clone(),
            to_primitive(timestamp),
            |inner, older_than| async move {
                use schema::proxies::dsl::*;

                let mut conn = inner.get_connection().await?;

                let vec = proxies
                    .select((accessed, alias))
                    .filter(accessed.lt(older_than))
                    .order(accessed.desc())
                    .limit(100)
                    .get_results(&mut conn)
                    .with_metrics("pict-rs.postgres.alias-access.older-aliases")
                    .with_timeout(Duration::from_secs(5))
                    .await
                    .map_err(|_| PostgresError::DbTimeout)?
                    .map_err(PostgresError::Diesel)?;

                Ok(vec)
            },
        )))
    }

    async fn remove_alias_access(&self, _: Alias) -> Result<(), RepoError> {
        // Noop - handled by ProxyRepo::remove_relation
        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl VariantAccessRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.variant-access.set-accessed")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.variant-access.accessed-at")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .optional()
            .map_err(PostgresError::Diesel)?
            .map(time::PrimitiveDateTime::assume_utc);

        Ok(opt)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn older_variants(
        &self,
        timestamp: time::OffsetDateTime,
    ) -> Result<LocalBoxStream<'static, Result<(Hash, String), RepoError>>, RepoError> {
        Ok(Box::pin(page_stream(
            self.inner.clone(),
            to_primitive(timestamp),
            |inner, older_than| async move {
                use schema::variants::dsl::*;

                let mut conn = inner.get_connection().await?;

                let vec = variants
                    .select((accessed, (hash, variant)))
                    .filter(accessed.lt(older_than))
                    .order(accessed.desc())
                    .limit(100)
                    .get_results(&mut conn)
                    .with_metrics("pict-rs.postgres.variant-access.older-variants")
                    .with_timeout(Duration::from_secs(5))
                    .await
                    .map_err(|_| PostgresError::DbTimeout)?
                    .map_err(PostgresError::Diesel)?;

                Ok(vec)
            },
        )))
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
    #[tracing::instrument(level = "debug", skip(self))]
    async fn create_upload(&self) -> Result<UploadId, RepoError> {
        use schema::uploads::dsl::*;

        let mut conn = self.get_connection().await?;

        let uuid = diesel::insert_into(uploads)
            .default_values()
            .returning(id)
            .get_result(&mut conn)
            .with_metrics("pict-rs.postgres.uploads.create")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(UploadId { id: uuid })
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn wait(&self, upload_id: UploadId) -> Result<UploadResult, RepoError> {
        let guard = WaitMetricsGuard::guard();
        use schema::uploads::dsl::*;

        let interest = self.inner.interest(upload_id);

        loop {
            tracing::trace!("wait: looping");

            let interest_future = interest.notified_timeout(Duration::from_secs(5));

            let mut notifier_conn = self.get_notifier_connection().await?;

            diesel::sql_query("LISTEN upload_completion_channel;")
                .execute(&mut notifier_conn)
                .with_metrics("pict-rs.postgres.uploads.listen")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
                .map_err(PostgresError::Diesel)?;

            drop(notifier_conn);

            let mut conn = self.get_connection().await?;

            let nested_opt = uploads
                .select(result)
                .filter(id.eq(upload_id.id))
                .get_result(&mut conn)
                .with_metrics("pict-rs.postgres.uploads.wait")
                .with_timeout(Duration::from_secs(5))
                .await
                .map_err(|_| PostgresError::DbTimeout)?
                .optional()
                .map_err(PostgresError::Diesel)?;

            match nested_opt {
                Some(opt) => {
                    if let Some(upload_result) = opt {
                        let upload_result: InnerUploadResult =
                            serde_json::from_value(upload_result)
                                .map_err(PostgresError::DeserializeUploadResult)?;

                        guard.disarm();
                        return Ok(upload_result.into());
                    }
                }
                None => {
                    return Err(RepoError::AlreadyClaimed);
                }
            }

            drop(conn);

            if interest_future.await.is_ok() {
                tracing::debug!("Notified");
            } else {
                tracing::debug!("Timed out");
            }
        }
    }

    #[tracing::instrument(level = "debug", skip(self))]
    async fn claim(&self, upload_id: UploadId) -> Result<(), RepoError> {
        use schema::uploads::dsl::*;

        let mut conn = self.get_connection().await?;

        diesel::delete(uploads)
            .filter(id.eq(upload_id.id))
            .execute(&mut conn)
            .with_metrics("pict-rs.postgres.uploads.claim")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    #[tracing::instrument(level = "debug", skip(self))]
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
            .with_metrics("pict-rs.postgres.uploads.complete")
            .with_timeout(Duration::from_secs(5))
            .await
            .map_err(|_| PostgresError::DbTimeout)?
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

#[async_trait::async_trait(?Send)]
impl FullRepo for PostgresRepo {
    #[tracing::instrument(level = "debug", skip(self))]
    async fn health_check(&self) -> Result<(), RepoError> {
        let next = self.inner.health_count.fetch_add(1, Ordering::Relaxed);

        self.set("health-value", Arc::from(next.to_be_bytes()))
            .await?;

        Ok(())
    }
}

fn page_stream<I, F, Fut>(
    inner: Arc<Inner>,
    mut older_than: time::PrimitiveDateTime,
    next: F,
) -> impl Stream<Item = Result<I, RepoError>>
where
    F: Fn(Arc<Inner>, time::PrimitiveDateTime) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<(time::PrimitiveDateTime, I)>, RepoError>>
        + 'static,
    I: 'static,
{
    streem::try_from_fn(|yielder| async move {
        loop {
            tracing::trace!("page_stream: looping");

            let mut page = (next)(inner.clone(), older_than).await?;

            if let Some((last_time, last_item)) = page.pop() {
                for (_, item) in page {
                    yielder.yield_ok(item).await;
                }

                yielder.yield_ok(last_item).await;

                older_than = last_time;
            } else {
                break;
            }
        }

        Ok(())
    })
}

impl std::fmt::Debug for PostgresRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRepo")
            .field("pool", &"pool")
            .finish()
    }
}
