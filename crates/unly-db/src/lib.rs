//! Database access layer for the unly agent platform.
//! Uses SQLite via sqlx with WAL mode and automatic migrations.

pub mod connection;
pub mod error;
pub mod repo;

pub use connection::{Database, DatabasePool};
pub use error::DbError;
