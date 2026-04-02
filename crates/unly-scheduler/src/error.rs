use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("database error: {0}")]
    Database(String),

    #[error("invalid cron expression: {0}")]
    InvalidCron(String),

    #[error("job not found: {0}")]
    NotFound(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
