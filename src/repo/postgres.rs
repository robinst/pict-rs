mod embedded {
    use refinery::embed_migrations;

    embed_migrations!("./src/repo/postgres/migrations");
}
mod schema;

use diesel_async::{
    pooled_connection::{
        deadpool::{BuildError, Pool},
        AsyncDieselConnectionManager,
    },
    AsyncPgConnection,
};
use url::Url;

use super::{BaseRepo, HashRepo};

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
enum PostgresError {}

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

impl BaseRepo for PostgresRepo {}

/*
#[async_trait::async_trait]
impl HashRepo for PostgresRepo {
    async fn size(&self) -> Result<u64, RepoError> {
        let conn = self.pool.get().await.map_err(PostgresError::from)?;
    }
}
*/

impl std::fmt::Debug for PostgresRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresRepo")
            .field("pool", &"pool")
            .finish()
    }
}
