//! Vector memory subsystem for the unly agent platform.
//!
//! Uses SQLite for storage with Rust-side cosine similarity computation.
//! Embeddings are stored as raw bytes (little-endian f32 sequences).

pub mod error;
pub mod memory;
pub mod scope;
pub mod similarity;

pub use error::MemoryError;
pub use memory::{MemoryEntry, MemoryQuery, MemoryResult, MemoryStore};
pub use scope::MemoryScope;
