use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("database error: {0}")]
    Database(#[from] unly_db::DbError),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("memory not found: {0}")]
    NotFound(String),
}
