use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use unly_agent::AgentContext;

/// Per-chat session store.
#[derive(Clone, Default)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<i64, AgentContext>>>,
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

    /// Number of active sessions.
    pub fn len(&self) -> usize {
        self.sessions.read().len()
    }
}
