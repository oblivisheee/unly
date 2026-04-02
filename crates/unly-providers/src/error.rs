use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("authentication error: {0}")]
    Auth(String),

    #[error("token expired, please re-authenticate")]
    TokenExpired,

    #[error("rate limit exceeded")]
    RateLimit,

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("request failed: {status} — {message}")]
    RequestFailed { status: u16, message: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("provider not configured: {0}")]
    NotConfigured(String),

    #[error("feature not supported: {feature} by provider {provider}")]
    NotSupported { provider: String, feature: String },
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

pub type ProviderResult<T> = std::result::Result<T, ProviderError>;
