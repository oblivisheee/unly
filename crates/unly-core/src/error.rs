use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("provider error: {provider} — {message}")]
    Provider { provider: String, message: String },

    #[error("provider not found: {0}")]
    ProviderNotFound(String),

    #[error("authentication error: {0}")]
    Auth(String),

    #[error("tool error: {tool} — {message}")]
    Tool { tool: String, message: String },

    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("tool execution denied: {reason}")]
    ToolDenied { reason: String },

    #[error("tool execution timed out: {tool}")]
    ToolTimeout { tool: String },

    #[error("memory error: {0}")]
    Memory(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("telegram error: {0}")]
    Telegram(String),

    #[error("scheduler error: {0}")]
    Scheduler(String),

    #[error("plugin error: {plugin} — {message}")]
    Plugin { plugin: String, message: String },

    #[error("plugin not found: {0}")]
    PluginNotFound(String),

    #[error("agent error: {0}")]
    Agent(String),

    #[error("subagent limit exceeded")]
    SubagentLimitExceeded,

    #[error("rate limit exceeded: {0}")]
    RateLimit(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn database(msg: impl Into<String>) -> Self {
        Self::Database(msg.into())
    }

    pub fn provider(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Provider {
            provider: provider.into(),
            message: message.into(),
        }
    }

    pub fn tool(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Tool {
            tool: tool.into(),
            message: message.into(),
        }
    }

    pub fn plugin(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Plugin {
            plugin: plugin.into(),
            message: message.into(),
        }
    }
}
