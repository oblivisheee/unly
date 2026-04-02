use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin not found: {0}")]
    NotFound(String),

    #[error("plugin already registered: {0}")]
    AlreadyRegistered(String),

    #[error("plugin incompatible: {plugin} requires platform >= {required}, got {current}")]
    Incompatible {
        plugin: String,
        required: String,
        current: String,
    },

    #[error("plugin initialization failed: {plugin} — {message}")]
    InitFailed { plugin: String, message: String },

    #[error("plugin shutdown failed: {plugin} — {message}")]
    ShutdownFailed { plugin: String, message: String },

    #[error("plugin is disabled: {0}")]
    Disabled(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
