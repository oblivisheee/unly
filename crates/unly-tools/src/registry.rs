use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use unly_core::{
    tool::{Tool, ToolContext, ToolResult},
    Error, Result,
};

use crate::policy::ExecutionPolicy;

/// Registry for all available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    enabled: Vec<String>,
    disabled: Vec<String>,
    policy: ExecutionPolicy,
    semaphore: Arc<Semaphore>,
}

impl ToolRegistry {
    pub fn new(policy: ExecutionPolicy, enabled: Vec<String>, disabled: Vec<String>) -> Self {
        let max_concurrent = policy.max_concurrent;
        Self {
            tools: HashMap::new(),
            enabled,
            disabled,
            policy,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: impl Tool + 'static) {
        let name = tool.schema().name.clone();
        info!("registering tool: {}", name);
        self.tools.insert(name, Arc::new(tool));
    }

    /// Check if a tool is accessible (enabled and not disabled).
    pub fn is_accessible(&self, name: &str) -> bool {
        if self.disabled.contains(&name.to_string()) {
            return false;
        }
        if self.enabled.is_empty() {
            return true; // empty = allow all that are not disabled
        }
        self.enabled.contains(&name.to_string())
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if !self.is_accessible(name) {
            return None;
        }
        self.tools.get(name).cloned()
    }

    /// List all accessible tool schemas.
    pub fn list_schemas(&self) -> Vec<unly_core::tool::ToolSchema> {
        self.tools
            .values()
            .filter(|t| self.is_accessible(&t.schema().name))
            .map(|t| t.schema())
            .collect()
    }

    /// Execute a tool, enforcing policy (timeouts, approval requirements).
    ///
    /// If the tool requires approval, returns `Error::ToolDenied` with a message
    /// indicating that approval is needed. The caller (agent runtime) should prompt
    /// the user and re-call with `force = true` after approval.
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: ToolContext,
        approved: bool,
    ) -> Result<ToolResult> {
        let tool = self
            .tools
            .get(name)
            .filter(|_| self.is_accessible(name))
            .ok_or_else(|| Error::ToolNotFound(name.to_string()))?
            .clone();

        let schema = tool.schema();

        // Policy check: approval required?
        if self.policy.needs_approval(&schema.risk) && !approved {
            return Err(Error::ToolDenied {
                reason: format!(
                    "tool '{}' is {:?} and requires explicit approval",
                    name, schema.risk
                ),
            });
        }

        let max_secs = self.policy.max_execution_seconds;
        let _permit = self.semaphore.acquire().await.map_err(|e| {
            Error::Tool {
                tool: name.to_string(),
                message: format!("semaphore error: {}", e),
            }
        })?;

        debug!(tool = %name, "executing tool");
        let start = Instant::now();

        let result = tokio::time::timeout(
            Duration::from_secs(max_secs),
            tool.execute(args, &ctx),
        )
        .await;

        match result {
            Ok(Ok(mut tool_result)) => {
                tool_result.duration_ms = start.elapsed().as_millis() as u64;
                Ok(tool_result)
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                warn!(tool = %name, error = %e, "tool execution failed");
                Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e.to_string(),
                    duration_ms,
                ))
            }
            Err(_) => {
                warn!(tool = %name, timeout_secs = %max_secs, "tool execution timed out");
                Err(Error::ToolTimeout {
                    tool: name.to_string(),
                })
            }
        }
    }

    /// Get the current execution policy.
    pub fn policy(&self) -> &ExecutionPolicy {
        &self.policy
    }
}
