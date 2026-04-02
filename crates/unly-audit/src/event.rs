use serde::{Deserialize, Serialize};
use unly_core::types::Timestamp;

/// Classification of audit event outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    Success,
    Failure,
    Denied,
}

impl std::fmt::Display for AuditOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditOutcome::Success => write!(f, "success"),
            AuditOutcome::Failure => write!(f, "failure"),
            AuditOutcome::Denied => write!(f, "denied"),
        }
    }
}

/// A structured audit event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_type: String,
    pub user_id: Option<String>,
    pub chat_id: Option<String>,
    pub agent_id: Option<String>,
    pub subject: String,
    pub action: String,
    pub outcome: AuditOutcome,
    pub details: serde_json::Value,
    pub timestamp: Timestamp,
}

impl AuditEvent {
    pub fn success(
        event_type: impl Into<String>,
        subject: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            user_id: None,
            chat_id: None,
            agent_id: None,
            subject: subject.into(),
            action: action.into(),
            outcome: AuditOutcome::Success,
            details: serde_json::Value::Null,
            timestamp: unly_core::types::now(),
        }
    }

    pub fn denied(
        event_type: impl Into<String>,
        subject: impl Into<String>,
        action: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            user_id: None,
            chat_id: None,
            agent_id: None,
            subject: subject.into(),
            action: action.into(),
            outcome: AuditOutcome::Denied,
            details: serde_json::json!({"reason": reason.into()}),
            timestamp: unly_core::types::now(),
        }
    }

    pub fn failure(
        event_type: impl Into<String>,
        subject: impl Into<String>,
        action: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            user_id: None,
            chat_id: None,
            agent_id: None,
            subject: subject.into(),
            action: action.into(),
            outcome: AuditOutcome::Failure,
            details: serde_json::json!({"error": error.into()}),
            timestamp: unly_core::types::now(),
        }
    }

    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn with_chat(mut self, chat_id: impl Into<String>) -> Self {
        self.chat_id = Some(chat_id.into());
        self
    }

    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }
}
