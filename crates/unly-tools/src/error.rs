use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),

    #[error("tool execution denied: {0}")]
    Denied(String),

    #[error("tool execution timed out after {seconds}s: {tool}")]
    Timeout { tool: String, seconds: u64 },

    #[error("tool execution failed: {tool} — {message}")]
    ExecutionFailed { tool: String, message: String },

    #[error("invalid arguments for tool {tool}: {message}")]
    InvalidArgs { tool: String, message: String },

    #[error("approval required for tool: {0}")]
    ApprovalRequired(String),

    #[error("policy error: {0}")]
    Policy(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
