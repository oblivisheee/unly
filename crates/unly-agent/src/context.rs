use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use unly_core::{
    ids::{AgentId, ChatId, UserId},
    model::ChatMessage,
    permissions::PermissionSet,
    types::Timestamp,
};

/// What kind of media to send to the Telegram chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Photo,
    Document,
    Video,
    Audio,
    Voice,
    Animation,
}

/// A media file queued for delivery to the Telegram chat by the tool runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSend {
    pub kind: MediaKind,
    /// Absolute path to the file on disk.
    pub path: String,
    pub caption: Option<String>,
}

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
    /// Per-session override for tool approval behavior.
    /// - Some(true): allow tool calls without approval prompts.
    /// - Some(false): require approval for all tool calls.
    /// - None: use global tool policy.
    pub tool_approval_override: Option<bool>,
    pub created_at: Timestamp,
    /// Accumulated reasoning/tool-use steps (Mode 1 — never shown to the user).
    pub thinking_log: Vec<ThinkingStep>,
    /// Media files queued for delivery after the current turn.
    pub pending_media: Vec<MediaSend>,
}

/// A step recorded in the agent's inner reasoning log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingStep {
    /// Short label for the step type (e.g. "tool_call", "tool_result", "reasoning").
    pub kind: String,
    /// Human-readable summary of the step.
    pub summary: String,
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
            tool_approval_override: None,
            created_at: unly_core::types::now(),
            thinking_log: Vec::new(),
            pending_media: Vec::new(),
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
        if let Some(mode_prompt) = self.tool_approval_mode_prompt() {
            msgs.push(ChatMessage {
                role: "system".to_string(),
                content: unly_core::model::ChatMessageContent::Text(mode_prompt),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            });
        }
        msgs.extend(self.messages.clone());
        msgs
    }

    /// Build messages with optional memory context injected as an extra system message.
    pub fn build_messages_with_memory(&self, memory_context: Option<&str>) -> Vec<ChatMessage> {
        let mut msgs = self.build_messages();
        if let Some(mem) = memory_context
            && !mem.trim().is_empty()
        {
            msgs.insert(
                1,
                ChatMessage {
                    role: "system".to_string(),
                    content: unly_core::model::ChatMessageContent::Text(mem.to_string()),
                    tool_call_id: None,
                    tool_calls: None,
                    name: None,
                },
            );
        }
        msgs
    }

    /// Push a message onto the context.
    pub fn push_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    /// Record a step in the internal thinking log.
    pub fn log_thinking(&mut self, kind: impl Into<String>, summary: impl Into<String>) {
        self.thinking_log.push(ThinkingStep {
            kind: kind.into(),
            summary: summary.into(),
        });
    }

    /// Trim messages to stay within a context window.
    pub fn trim_to(&mut self, max_messages: usize) {
        if self.messages.len() > max_messages {
            let remove = self.messages.len() - max_messages;
            self.messages.drain(0..remove);
        }
        self.prune_orphan_tool_messages();
    }

    /// Drop `tool` messages that no longer have a matching preceding
    /// assistant `tool_calls` entry (can happen after context trimming).
    fn prune_orphan_tool_messages(&mut self) {
        let mut known_tool_calls: HashSet<String> = HashSet::new();
        let mut normalized = Vec::with_capacity(self.messages.len());

        for msg in self.messages.drain(..) {
            if msg.role == "assistant" {
                if let Some(calls) = msg.tool_calls.as_ref() {
                    for call in calls {
                        known_tool_calls.insert(call.id.clone());
                    }
                }
                normalized.push(msg);
                continue;
            }

            if msg.role == "tool" {
                let Some(tool_call_id) = msg.tool_call_id.as_ref() else {
                    continue;
                };
                if !known_tool_calls.contains(tool_call_id) {
                    continue;
                }
            }

            normalized.push(msg);
        }

        self.messages = normalized;
    }

    fn tool_approval_mode_prompt(&self) -> Option<String> {
        match self.tool_approval_override {
            Some(true) => Some(
                "Tool approval mode is AUTO. If tools are needed, call them directly without asking the user for permission in plain text."
                    .to_string(),
            ),
            Some(false) => Some(
                "Tool approval mode is MANUAL. If tools are needed, call them directly and let the runtime handle approve/deny prompts. Do not ask for permission in plain text."
                    .to_string(),
            ),
            None => None,
        }
    }
}
