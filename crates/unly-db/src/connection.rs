use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::Path;
use tracing::info;

use crate::error::DbResult;

/// Type alias for the SQLite connection pool.
pub type DatabasePool = SqlitePool;

/// Database handle providing access to the connection pool and migrations.
#[derive(Clone, Debug)]
pub struct Database {
    pool: DatabasePool,
}

impl Database {
    /// Connect to the SQLite database at the given path and run migrations.
    pub async fn connect(
        db_path: impl AsRef<Path>,
        max_connections: u32,
        auto_migrate: bool,
    ) -> DbResult<Self> {
        let path = db_path.as_ref();

        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crate::error::DbError::Config(format!(
                    "failed to create database directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let db_url = format!("sqlite://{}?mode=rwc", path.display());

        info!("connecting to database at {}", path.display());

        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect(&db_url)
            .await?;

        // Enable WAL mode and foreign keys.
        sqlx::query("PRAGMA journal_mode=WAL").execute(&pool).await?;
        sqlx::query("PRAGMA foreign_keys=ON").execute(&pool).await?;
        sqlx::query("PRAGMA synchronous=NORMAL").execute(&pool).await?;

        let db = Self { pool };

        if auto_migrate {
            db.migrate().await?;
        }

        Ok(db)
    }

    /// Run all pending database migrations.
    pub async fn migrate(&self) -> DbResult<()> {
        info!("running database migrations");
        // Use runtime migration from embedded SQL.
        let migrator = sqlx::migrate!("../../migrations");
        migrator
            .run(&self.pool)
            .await
            .map_err(crate::error::DbError::Migration)?;
        info!("database migrations complete");
        Ok(())
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &DatabasePool {
        &self.pool
    }

    /// Close the database connection pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Check database connectivity.
    pub async fn health_check(&self) -> DbResult<()> {
        sqlx::query("SELECT 1").fetch_one(&self.pool).await?;
        Ok(())
    }
}
