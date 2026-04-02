//! SeaORM-based database access layer for the unly agent platform.
//!
//! This crate provides:
//! - [`Database`]: the main connection handle (SeaORM `DatabaseConnection`)
//! - [`entity`]: SeaORM entity definitions for every table
//! - [`migration`]: SeaORM `MigrationTrait` structs (replaces raw SQL files)
//! - [`repo`]: typed repository layer built on top of the entities

pub mod connection;
pub mod entity;
pub mod error;
pub mod migration;
pub mod repo;

pub use connection::Database;
pub use error::{DbError, DbResult};
