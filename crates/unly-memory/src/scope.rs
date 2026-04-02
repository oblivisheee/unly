use serde::{Deserialize, Serialize};

/// Memory scope type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Memories associated with a specific user.
    User(String),
    /// Memories associated with a specific chat.
    Chat(String),
    /// Memories associated with a specific agent run.
    Agent(String),
    /// Memories associated with a specific subagent.
    Subagent(String),
}

impl MemoryScope {
    pub fn scope_type(&self) -> &str {
        match self {
            MemoryScope::User(_) => "user",
            MemoryScope::Chat(_) => "chat",
            MemoryScope::Agent(_) => "agent",
            MemoryScope::Subagent(_) => "subagent",
        }
    }

    pub fn scope_id(&self) -> &str {
        match self {
            MemoryScope::User(id)
            | MemoryScope::Chat(id)
            | MemoryScope::Agent(id)
            | MemoryScope::Subagent(id) => id,
        }
    }
}
