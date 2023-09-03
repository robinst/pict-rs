mod embedded;
mod schema;

use std::sync::Arc;

use diesel::prelude::*;
use diesel_async::{
    pooled_connection::{
        deadpool::{BuildError, Pool, PoolError},
        AsyncDieselConnectionManager, ManagerConfig,
    },
    AsyncConnection, AsyncPgConnection, RunQueryDsl,
};
use tokio_postgres::{AsyncMessage, Notification};
use url::Url;

use crate::error_code::ErrorCode;

use super::{
    BaseRepo, Hash, HashAlreadyExists, HashPage, HashRepo, OrderedHash, RepoError,
    VariantAlreadyExists,
};

#[derive(Clone)]
pub(crate) struct PostgresRepo {
    pool: Pool<AsyncPgConnection>,
    notifications: flume::Receiver<Notification>,
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
}

impl PostgresError {
    pub(super) const fn error_code(&self) -> ErrorCode {
        todo!()
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

        let (tx, notifications) = flume::bounded(10);

        let mut config = ManagerConfig::default();
        config.custom_setup = build_handler(tx);

        let mgr = AsyncDieselConnectionManager::<AsyncPgConnection>::new_with_config(
            postgres_url,
            config,
        );
        let pool = Pool::builder(mgr)
            .build()
            .map_err(ConnectPostgresError::BuildPool)?;

        Ok(PostgresRepo {
            pool,
            notifications,
        })
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
    async fn size(&self) -> Result<u64, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

        let count = hashes
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(count.try_into().expect("non-negative count"))
    }

    async fn bound(&self, input_hash: Hash) -> Result<Option<OrderedHash>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn hash_page_by_date(
        &self,
        date: time::OffsetDateTime,
        limit: usize,
    ) -> Result<HashPage, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn hashes_ordered(
        &self,
        bound: Option<OrderedHash>,
        limit: usize,
    ) -> Result<HashPage, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn create_hash_with_timestamp(
        &self,
        input_hash: Hash,
        input_identifier: &Arc<str>,
        timestamp: time::OffsetDateTime,
    ) -> Result<Result<(), HashAlreadyExists>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn update_identifier(
        &self,
        input_hash: Hash,
        input_identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

        diesel::update(hashes)
            .filter(hash.eq(&input_hash))
            .set(identifier.eq(input_identifier.as_ref()))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    async fn identifier(&self, input_hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

        let opt = hashes
            .select(identifier)
            .filter(hash.eq(&input_hash))
            .get_result::<String>(&mut conn)
            .await
            .optional()
            .map_err(PostgresError::Diesel)?;

        Ok(opt.map(Arc::from))
    }

    async fn relate_variant_identifier(
        &self,
        input_hash: Hash,
        input_variant: String,
        input_identifier: &Arc<str>,
    ) -> Result<Result<(), VariantAlreadyExists>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn variant_identifier(
        &self,
        input_hash: Hash,
        input_variant: String,
    ) -> Result<Option<Arc<str>>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn variants(&self, input_hash: Hash) -> Result<Vec<(String, Arc<str>)>, RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn remove_variant(
        &self,
        input_hash: Hash,
        input_variant: String,
    ) -> Result<(), RepoError> {
        use schema::variants::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

        diesel::delete(variants)
            .filter(hash.eq(&input_hash))
            .filter(variant.eq(&input_variant))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    async fn relate_motion_identifier(
        &self,
        input_hash: Hash,
        input_identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

        diesel::update(hashes)
            .filter(hash.eq(&input_hash))
            .set(motion_identifier.eq(input_identifier.as_ref()))
            .execute(&mut conn)
            .await
            .map_err(PostgresError::Diesel)?;

        Ok(())
    }

    async fn motion_identifier(&self, input_hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        use schema::hashes::dsl::*;

        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

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

    async fn cleanup_hash(&self, input_hash: Hash) -> Result<(), RepoError> {
        let mut conn = self.pool.get().await.map_err(PostgresError::Pool)?;

        conn.transaction(|conn| {
            Box::pin(async move {
                diesel::delete(schema::hashes::dsl::hashes)
                    .filter(schema::hashes::dsl::hash.eq(&input_hash))
                    .execute(conn)
                    .await?;

                diesel::delete(schema::variants::dsl::variants)
                    .filter(schema::variants::dsl::hash.eq(&input_hash))
                    .execute(conn)
                    .await
            })
        })
        .await
        .map_err(PostgresError::Diesel)?;

        Ok(())
    }
}

impl std::fmt::Debug for PostgresRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRepo")
            .field("pool", &"pool")
            .finish()
    }
}
