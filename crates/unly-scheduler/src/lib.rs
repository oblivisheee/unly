//! Scheduler subsystem for the unly agent platform.
//!
//! Manages cron-based and event-triggered background jobs persisted in SQLite.

pub mod error;
pub mod job;
pub mod scheduler;

pub use error::SchedulerError;
pub use job::{JobDefinition, JobType};
pub use scheduler::{JobCallback, Scheduler};
