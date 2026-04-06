use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use unly_agent::AgentContext;
use unly_core::ids::AgentId;

/// Per-chat session store.
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<i64, SessionEntry>>>,
    idle_timeout: Duration,
    global_auto_approve: Arc<RwLock<Option<bool>>>,
    pending_subagents: Arc<RwLock<HashMap<i64, PendingSubagentSpawn>>>,
    skip_history_restore_once: Arc<RwLock<HashMap<i64, bool>>>,
}

#[derive(Clone)]
struct SessionEntry {
    ctx: AgentContext,
    last_touched: Instant,
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
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            idle_timeout,
            global_auto_approve: Arc::new(RwLock::new(None)),
            pending_subagents: Arc::new(RwLock::new(HashMap::new())),
            skip_history_restore_once: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or insert a session for the given Telegram chat ID.
    pub fn get(&self, chat_id: i64) -> Option<AgentContext> {
        let now = Instant::now();
        let mut sessions = self.sessions.write();
        if let Some(entry) = sessions.get(&chat_id)
            && now.duration_since(entry.last_touched) > self.idle_timeout
        {
            sessions.remove(&chat_id);
            self.pending_subagents.write().remove(&chat_id);
            self.skip_history_restore_once.write().remove(&chat_id);
            return None;
        }
        let entry = sessions.get_mut(&chat_id)?;
        entry.last_touched = now;
        Some(entry.ctx.clone())
    }

    /// Insert or replace a session.
    pub fn set(&self, chat_id: i64, ctx: AgentContext) {
        self.sessions.write().insert(
            chat_id,
            SessionEntry {
                ctx,
                last_touched: Instant::now(),
            },
        );
    }

    /// Remove a session (reset chat).
    pub fn remove(&self, chat_id: i64) -> bool {
        self.pending_subagents.write().remove(&chat_id);
        self.sessions.write().remove(&chat_id).is_some()
    }

    pub fn global_auto_approve(&self) -> Option<bool> {
        *self.global_auto_approve.read()
    }

    pub fn set_global_auto_approve(&self, value: bool) {
        *self.global_auto_approve.write() = Some(value);
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

    pub fn mark_skip_history_restore(&self, chat_id: i64) {
        self.skip_history_restore_once.write().insert(chat_id, true);
    }

    pub fn take_skip_history_restore(&self, chat_id: i64) -> bool {
        self.skip_history_restore_once
            .write()
            .remove(&chat_id)
            .unwrap_or(false)
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new(Duration::from_secs(6 * 60 * 60))
    }
}
