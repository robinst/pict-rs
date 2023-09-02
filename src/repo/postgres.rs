mod embedded;
mod schema;

use std::sync::Arc;

use diesel::prelude::*;
use diesel_async::{
    pooled_connection::{
        deadpool::{BuildError, Pool, PoolError},
        AsyncDieselConnectionManager,
    },
    AsyncPgConnection, RunQueryDsl,
};
use url::Url;

use crate::error_code::ErrorCode;

use super::{
    BaseRepo, Hash, HashAlreadyExists, HashPage, HashRepo, OrderedHash, RepoError,
    VariantAlreadyExists,
};

#[derive(Clone)]
pub(crate) struct PostgresRepo {
    pool: Pool<AsyncPgConnection>,
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

        let config = AsyncDieselConnectionManager::<AsyncPgConnection>::new(postgres_url);
        let pool = Pool::builder(config)
            .build()
            .map_err(ConnectPostgresError::BuildPool)?;

        Ok(PostgresRepo { pool })
    }
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

        /*
        insert_into(hashes).values((
            hash.eq(&input_hash),
            identifier.eq(&input_identifier)
        ))
        */

        todo!()
    }

    async fn update_identifier(&self, hash: Hash, identifier: &Arc<str>) -> Result<(), RepoError> {
        todo!()
    }

    async fn identifier(&self, hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        todo!()
    }

    async fn relate_variant_identifier(
        &self,
        hash: Hash,
        variant: String,
        identifier: &Arc<str>,
    ) -> Result<Result<(), VariantAlreadyExists>, RepoError> {
        todo!()
    }

    async fn variant_identifier(
        &self,
        hash: Hash,
        variant: String,
    ) -> Result<Option<Arc<str>>, RepoError> {
        todo!()
    }

    async fn variants(&self, hash: Hash) -> Result<Vec<(String, Arc<str>)>, RepoError> {
        todo!()
    }

    async fn remove_variant(&self, hash: Hash, variant: String) -> Result<(), RepoError> {
        todo!()
    }

    async fn relate_motion_identifier(
        &self,
        hash: Hash,
        identifier: &Arc<str>,
    ) -> Result<(), RepoError> {
        todo!()
    }

    async fn motion_identifier(&self, hash: Hash) -> Result<Option<Arc<str>>, RepoError> {
        todo!()
    }

    async fn cleanup_hash(&self, hash: Hash) -> Result<(), RepoError> {
        todo!()
    }
}

impl std::fmt::Debug for PostgresRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRepo")
            .field("pool", &"pool")
            .finish()
    }
}
