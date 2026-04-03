use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use unly_agent::AgentContext;
use unly_core::ids::AgentId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SessionFlags {
    pub auto_approve: bool,
}

/// Per-chat session store.
#[derive(Clone, Default)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<i64, AgentContext>>>,
    flags: Arc<RwLock<HashMap<i64, SessionFlags>>>,
    pending_subagents: Arc<RwLock<HashMap<i64, PendingSubagentSpawn>>>,
}

#[derive(Debug, Clone)]
pub struct PendingSubagentSpawn {
    pub goal: String,
    pub parent_agent_id: AgentId,
    pub depth: u32,
    pub provider: String,
    pub model: String,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get or insert a session for the given Telegram chat ID.
    pub fn get(&self, chat_id: i64) -> Option<AgentContext> {
        self.sessions.read().get(&chat_id).cloned()
    }

    /// Insert or replace a session.
    pub fn set(&self, chat_id: i64, ctx: AgentContext) {
        self.sessions.write().insert(chat_id, ctx);
    }

    /// Remove a session (reset chat).
    pub fn remove(&self, chat_id: i64) -> bool {
        self.sessions.write().remove(&chat_id).is_some()
    }

    pub fn get_flags(&self, chat_id: i64) -> SessionFlags {
        self.flags.read().get(&chat_id).copied().unwrap_or_default()
    }

    pub fn set_auto_approve(&self, chat_id: i64, value: bool) {
        let mut flags = self.flags.write();
        let mut current = flags.get(&chat_id).copied().unwrap_or_default();
        current.auto_approve = value;
        flags.insert(chat_id, current);
    }

    /// Number of active sessions.
    pub fn len(&self) -> usize {
        self.sessions.read().len()
    }

    /// Whether there are no active sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.read().is_empty()
    }

    pub fn set_pending_subagent(&self, chat_id: i64, pending: PendingSubagentSpawn) {
        self.pending_subagents.write().insert(chat_id, pending);
    }

    pub fn take_pending_subagent(&self, chat_id: i64) -> Option<PendingSubagentSpawn> {
        self.pending_subagents.write().remove(&chat_id)
    }

    pub fn has_pending_subagent(&self, chat_id: i64) -> bool {
        self.pending_subagents.read().contains_key(&chat_id)
    }
}
