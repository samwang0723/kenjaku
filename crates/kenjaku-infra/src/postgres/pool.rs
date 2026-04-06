use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use kenjaku_core::config::PostgresConfig;
use kenjaku_core::error::{Error, Result};

/// Create a PostgreSQL connection pool.
pub async fn create_pool(config: &PostgresConfig) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .connect(&config.url)
        .await
        .map_err(|e| Error::Database(format!("Failed to create PG pool: {e}")))
}

/// Run database migrations from the migrations directory.
/// Uses runtime migration loading instead of compile-time `migrate!` macro
/// so that compilation does not require DATABASE_URL.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    let migrator = sqlx::migrate::Migrator::new(std::path::Path::new("./migrations"))
        .await
        .map_err(|e| Error::Database(format!("Failed to load migrations: {e}")))?;

    migrator
        .run(pool)
        .await
        .map_err(|e| Error::Database(format!("Migration failed: {e}")))
}

/// Check if PostgreSQL is healthy.
pub async fn health_check(pool: &PgPool) -> Result<()> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .map_err(|e| Error::Database(format!("PG health check failed: {e}")))?;
    Ok(())
}
