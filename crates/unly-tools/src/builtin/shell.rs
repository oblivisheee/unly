use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use unly_core::{
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
    Result,
};

/// Tool: execute a shell command.
///
/// IMPORTANT: This tool is Dangerous and always requires approval.
/// The command must match the configured shell_allowlist policy.
/// Executed via /bin/sh -c with restricted environment.
pub struct ShellTool {
    allowlist: Vec<String>,
    working_dir: Option<PathBuf>,
}

impl ShellTool {
    pub fn new(allowlist: Vec<String>, working_dir: Option<PathBuf>) -> Self {
        Self {
            allowlist,
            working_dir,
        }
    }

    fn is_allowed(&self, command: &str) -> bool {
        if self.allowlist.is_empty() {
            return false;
        }
        for pattern in &self.allowlist {
            if let Ok(re) = regex::Regex::new(pattern) {
                if re.is_match(command) {
                    return true;
                }
            }
        }
        false
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "shell".to_string(),
            description: "Execute a shell command. Requires approval. Only allowlisted commands are permitted.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Optional working directory for the command."
                    }
                },
                "required": ["command"]
            }),
            risk: ToolRisk::Dangerous,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let command = args["command"].as_str().ok_or_else(|| {
            unly_core::Error::InvalidInput("missing command argument".to_string())
        })?;

        // Policy: check allowlist.
        if !self.is_allowed(command) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("command not in allowlist: {}", command),
                start.elapsed().as_millis() as u64,
            ));
        }

        let working_dir = args["working_dir"]
            .as_str()
            .map(PathBuf::from)
            .or_else(|| self.working_dir.clone());

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        // Restricted environment: only essential vars.
        cmd.env_clear();
        cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code();
                let duration_ms = start.elapsed().as_millis() as u64;
                let is_error = !output.status.success();
                Ok(ToolResult {
                    tool_call_id: ctx.tool_call_id.clone(),
                    stdout,
                    stderr,
                    exit_code,
                    is_error,
                    duration_ms,
                    metadata: Value::Null,
                })
            }
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}
