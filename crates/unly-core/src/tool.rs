use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::error::Result;

/// Classification of tool risk level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolRisk {
    /// Safe, read-only, no side effects.
    Safe,
    /// Has side effects but not destructive.
    Privileged,
    /// Destructive or security-sensitive.
    Dangerous,
}

/// Input schema for a tool, expressed as JSON Schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub risk: ToolRisk,
    pub requires_approval: bool,
}

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub is_error: bool,
    pub duration_ms: u64,
    pub metadata: Value,
}

impl ToolResult {
    pub fn success(tool_call_id: impl Into<String>, stdout: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: Some(0),
            is_error: false,
            duration_ms,
            metadata: Value::Null,
        }
    }

    pub fn error(tool_call_id: impl Into<String>, message: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            stdout: String::new(),
            stderr: message.into(),
            exit_code: Some(1),
            is_error: true,
            duration_ms,
            metadata: Value::Null,
        }
    }
}

/// Context provided to a tool during execution.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub tool_call_id: String,
    pub user_id: Option<crate::ids::UserId>,
    pub chat_id: Option<crate::ids::ChatId>,
    pub agent_id: Option<crate::ids::AgentId>,
}

/// A tool that can be registered and invoked.
#[async_trait]
pub trait Tool: Send + Sync {
    fn schema(&self) -> ToolSchema;

    async fn execute(
        &self,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult>;
}
