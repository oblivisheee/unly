//! Core domain model, shared types, traits, and errors for the unly agent platform.

pub mod error;
pub mod ids;
pub mod message;
pub mod model;
pub mod permissions;
pub mod provider;
pub mod tool;
pub mod types;

pub use error::{Error, Result};
pub use ids::*;
pub use types::*;
