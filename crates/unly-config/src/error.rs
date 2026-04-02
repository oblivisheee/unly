use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration file not found: {path}")]
    NotFound { path: String },

    #[error("configuration parse error: {0}")]
    Parse(String),

    #[error("configuration validation error: {field} — {message}")]
    Validation { field: String, message: String },

    #[error("missing required field: {0}")]
    MissingField(String),

    #[error("environment variable error: {var} — {message}")]
    EnvVar { var: String, message: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
