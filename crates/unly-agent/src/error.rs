use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("provider error: {0}")]
    Provider(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("memory error: {0}")]
    Memory(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("subagent depth limit exceeded")]
    SubagentDepthExceeded,

    #[error("subagent limit exceeded")]
    SubagentLimitExceeded,

    #[error("token budget exceeded")]
    TokenBudgetExceeded,

    #[error("max turns exceeded")]
    MaxTurnsExceeded,

    #[error("approval required for: {0}")]
    ApprovalRequired(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}
