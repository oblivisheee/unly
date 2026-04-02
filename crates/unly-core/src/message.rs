use serde::{Deserialize, Serialize};
use crate::ids::{ChatId, MessageId, UserId};
use crate::types::{Role, Timestamp};

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub chat_id: ChatId,
    pub user_id: Option<UserId>,
    pub role: Role,
    pub content: MessageContent,
    pub created_at: Timestamp,
    pub metadata: serde_json::Value,
}

/// Content variants for messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
    Text { text: String },
    ImageUrl { url: String, detail: Option<String> },
    Document { file_id: String, file_name: Option<String>, caption: Option<String> },
    ToolCall { tool_call_id: String, function_name: String, arguments: serde_json::Value },
    ToolResult { tool_call_id: String, content: String, is_error: bool },
    Mixed { parts: Vec<ContentPart> },
}

/// A single content part inside a mixed message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { url: String },
}

impl MessageContent {
    /// Extract plain text representation.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Convert to text regardless of variant, for display/logging.
    pub fn display_text(&self) -> String {
        match self {
            MessageContent::Text { text } => text.clone(),
            MessageContent::ImageUrl { url, .. } => format!("[image: {}]", url),
            MessageContent::Document { file_name, .. } => {
                format!("[document: {}]", file_name.as_deref().unwrap_or("unknown"))
            }
            MessageContent::ToolCall { function_name, .. } => {
                format!("[tool call: {}]", function_name)
            }
            MessageContent::ToolResult { content, is_error, .. } => {
                if *is_error {
                    format!("[tool error: {}]", content)
                } else {
                    format!("[tool result: {}]", content)
                }
            }
            MessageContent::Mixed { parts } => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// A conversation chat context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: ChatId,
    pub telegram_chat_id: Option<i64>,
    pub title: Option<String>,
    pub system_prompt: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub metadata: serde_json::Value,
}
