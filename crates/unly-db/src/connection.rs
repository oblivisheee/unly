use sea_orm::{ConnectOptions, Database as SeaDatabase, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use std::path::Path;
use std::time::Duration;
use tracing::info;

use unly_config::config::{DatabaseConfig, DbType};

use crate::error::{DbError, DbResult};
use crate::migration::Migrator;

/// Database handle wrapping a SeaORM `DatabaseConnection`.
#[derive(Clone, Debug)]
pub struct Database {
    conn: DatabaseConnection,
    db_type: DbType,
}

impl Database {
    /// Connect to the database using the provided [`DatabaseConfig`].
    ///
    /// Automatically selects the correct backend (SQLite or PostgreSQL) and
    /// runs pending migrations when `auto_migrate` is set.
    pub async fn connect_with_config(config: &DatabaseConfig) -> DbResult<Self> {
        match config.db_type {
            DbType::Sqlite => {
                Self::connect_sqlite(&config.path, config.max_connections, config.auto_migrate)
                    .await
            }
            DbType::Postgres => {
                let url = config.postgres_url.as_deref().ok_or_else(|| {
                    DbError::Config(
                        "postgres_url must be set when db_type = \"postgres\"".to_string(),
                    )
                })?;
                Self::connect_postgres(url, config.max_connections, config.auto_migrate).await
            }
        }
    }

    /// Open (or create) the SQLite database and optionally run all pending
    /// migrations.
    pub async fn connect(
        db_path: impl AsRef<Path>,
        max_connections: u32,
        auto_migrate: bool,
    ) -> DbResult<Self> {
        Self::connect_sqlite(db_path, max_connections, auto_migrate).await
    }

    async fn connect_sqlite(
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
        info!("connecting to SQLite database at {}", path.display());

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

        let db = Self {
            conn,
            db_type: DbType::Sqlite,
        };

        if auto_migrate {
            db.migrate().await?;
        }

        Ok(db)
    }

    async fn connect_postgres(
        url: &str,
        max_connections: u32,
        auto_migrate: bool,
    ) -> DbResult<Self> {
        info!("connecting to PostgreSQL database");

        let mut opts = ConnectOptions::new(url.to_string());
        opts.max_connections(max_connections)
            .min_connections(1)
            .connect_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(300))
            .sqlx_logging(false);

        let conn = SeaDatabase::connect(opts).await?;

        let db = Self {
            conn,
            db_type: DbType::Postgres,
        };

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

    /// Return the active database backend type.
    pub fn db_type(&self) -> &DbType {
        &self.db_type
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
