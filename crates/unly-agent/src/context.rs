use serde::{Deserialize, Serialize};
use unly_core::{
    ids::{AgentId, ChatId, UserId},
    model::ChatMessage,
    permissions::PermissionSet,
    types::Timestamp,
};

/// The runtime context for an agent interaction.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub agent_id: AgentId,
    pub chat_id: ChatId,
    pub user_id: Option<UserId>,
    pub permissions: PermissionSet,
    pub provider: String,
    pub model: String,
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>,
    pub turn_count: u32,
    pub subagent_depth: u32,
    pub pending_approvals: Vec<PendingApproval>,
    pub created_at: Timestamp,
}

/// A pending tool call that awaits user approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub risk_level: String,
}

impl AgentContext {
    pub fn new(
        chat_id: ChatId,
        user_id: Option<UserId>,
        permissions: PermissionSet,
        provider: impl Into<String>,
        model: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: AgentId::new(),
            chat_id,
            user_id,
            permissions,
            provider: provider.into(),
            model: model.into(),
            system_prompt: system_prompt.into(),
            messages: Vec::new(),
            turn_count: 0,
            subagent_depth: 0,
            pending_approvals: Vec::new(),
            created_at: unly_core::types::now(),
        }
    }

    /// Build the full message list including system prompt.
    pub fn build_messages(&self) -> Vec<ChatMessage> {
        let mut msgs = vec![ChatMessage {
            role: "system".to_string(),
            content: unly_core::model::ChatMessageContent::Text(self.system_prompt.clone()),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        }];
        msgs.extend(self.messages.clone());
        msgs
    }

    /// Push a message onto the context.
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    /// Trim messages to stay within a context window.
    pub fn trim_to(&mut self, max_messages: usize) {
        if self.messages.len() > max_messages {
            let remove = self.messages.len() - max_messages;
            self.messages.drain(0..remove);
        }
    }
}
