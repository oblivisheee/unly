use serde::{Deserialize, Serialize};
use unly_core::tool::ToolRisk;

/// Policy governing tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    /// Whether privileged tools require user approval before execution.
    pub require_approval_for_privileged: bool,
    /// Whether dangerous tools require user approval before execution.
    pub require_approval_for_dangerous: bool,
    /// Maximum execution time in seconds.
    pub max_execution_seconds: u64,
    /// Maximum concurrent tool executions.
    pub max_concurrent: usize,
    /// Shell command allowlist (regex patterns). Empty = deny all.
    pub shell_allowlist: Vec<String>,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            require_approval_for_privileged: true,
            require_approval_for_dangerous: true,
            max_execution_seconds: 30,
            max_concurrent: 4,
            shell_allowlist: Vec::new(),
        }
    }
}

impl ExecutionPolicy {
    /// Check if a tool with the given risk needs approval.
    pub fn needs_approval(&self, risk: &ToolRisk) -> bool {
        match risk {
            ToolRisk::Safe => false,
            ToolRisk::Privileged => self.require_approval_for_privileged,
            ToolRisk::Dangerous => self.require_approval_for_dangerous,
        }
    }

    /// Check if a shell command is allowed.
    pub fn is_shell_allowed(&self, command: &str) -> bool {
        if self.shell_allowlist.is_empty() {
            return false;
        }
        for pattern in &self.shell_allowlist {
            if let Ok(re) = regex::Regex::new(pattern) {
                if re.is_match(command) {
                    return true;
                }
            }
        }
        false
    }
}

/// Per-tool policy override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub name: String,
    pub enabled: bool,
    pub override_risk: Option<ToolRisk>,
    pub require_approval: Option<bool>,
    pub max_execution_seconds: Option<u64>,
}
