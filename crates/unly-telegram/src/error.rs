use thiserror::Error;

#[derive(Debug, Error)]
pub enum TelegramError {
    #[error("bot error: {0}")]
    Bot(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("user blocked")]
    Blocked,

    #[error("rate limit exceeded")]
    RateLimit,

    #[error("unknown command: {0}")]
    UnknownCommand(String),

    #[error("session error: {0}")]
    Session(String),
}
