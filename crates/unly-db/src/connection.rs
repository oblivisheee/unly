use sea_orm::{ConnectOptions, Database as SeaDatabase, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use std::path::Path;
use std::time::Duration;
use tracing::info;

use crate::error::{DbError, DbResult};
use crate::migration::Migrator;

/// Database handle wrapping a SeaORM `DatabaseConnection`.
#[derive(Clone, Debug)]
pub struct Database {
    conn: DatabaseConnection,
}

impl Database {
    /// Open (or create) the SQLite database and optionally run all pending
    /// migrations.
    pub async fn connect(
        db_path: impl AsRef<Path>,
        max_connections: u32,
        auto_migrate: bool,
    ) -> DbResult<Self> {
        let path = db_path.as_ref();

        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    DbError::Config(format!(
                        "failed to create database directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }

        let db_url = format!("sqlite://{}?mode=rwc", path.display());
        info!("connecting to database at {}", path.display());

        let mut opts = ConnectOptions::new(db_url);
        opts.max_connections(max_connections)
            .min_connections(1)
            .connect_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(300))
            .sqlx_logging(false);

        let conn = SeaDatabase::connect(opts).await?;

        // Enable WAL mode and foreign keys via raw statement (SQLite pragmas).
        use sea_orm::ConnectionTrait;
        conn.execute_unprepared("PRAGMA journal_mode=WAL").await?;
        conn.execute_unprepared("PRAGMA foreign_keys=ON").await?;
        conn.execute_unprepared("PRAGMA synchronous=NORMAL").await?;

        let db = Self { conn };

        if auto_migrate {
            db.migrate().await?;
        }

        Ok(db)
    }

    /// Run all pending SeaORM migrations.
    pub async fn migrate(&self) -> DbResult<()> {
        info!("running database migrations");
        Migrator::up(&self.conn, None).await?;
        info!("database migrations complete");
        Ok(())
    }

    /// Get a reference to the underlying SeaORM `DatabaseConnection`.
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    /// Close the database connection gracefully.
    pub async fn close(self) {
        self.conn.close().await.ok();
    }

    /// Verify the database is reachable.
    pub async fn health_check(&self) -> DbResult<()> {
        use sea_orm::ConnectionTrait;
        self.conn.execute_unprepared("SELECT 1").await?;
        Ok(())
    }
}
