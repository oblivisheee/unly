use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

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
    requires_approval: bool,
}

impl ShellTool {
    pub fn new(
        allowlist: Vec<String>,
        working_dir: Option<PathBuf>,
        requires_approval: bool,
    ) -> Self {
        Self {
            allowlist,
            working_dir,
            requires_approval,
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

    fn resolve_working_dir(&self, args: &Value) -> Option<PathBuf> {
        args["working_dir"]
            .as_str()
            .map(PathBuf::from)
            .or_else(|| self.working_dir.clone())
    }

    async fn execute_with_program(
        &self,
        program: &str,
        program_args: &[&str],
        command: &str,
        working_dir: Option<PathBuf>,
        ctx: &ToolContext,
        start: Instant,
    ) -> ToolResult {
        let mut cmd = Command::new(program);
        cmd.args(program_args).arg(command);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // Restricted environment: only essential vars.
        cmd.env_clear();
        cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        match cmd.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code();
                let duration_ms = start.elapsed().as_millis() as u64;
                let is_error = !output.status.success();
                ToolResult {
                    tool_call_id: ctx.tool_call_id.clone(),
                    stdout,
                    stderr,
                    exit_code,
                    is_error,
                    duration_ms,
                    metadata: Value::Null,
                }
            }
            Err(e) => ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            ),
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "shell".to_string(),
            description: if self.requires_approval {
                "Execute a shell command. Requires approval. Only allowlisted commands are permitted."
            } else {
                "Execute a shell command. Only allowlisted commands are permitted."
            }
            .to_string(),
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
            requires_approval: self.requires_approval,
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

        let working_dir = self.resolve_working_dir(&args);
        Ok(self
            .execute_with_program("/bin/sh", &["-c"], command, working_dir, ctx, start)
            .await)
    }
}

/// `bash` alias for the shell tool.
///
/// Uses the same execution and policy checks as `shell`, but with a clearer
/// name for users expecting a bash-like command tool.
pub struct BashTool {
    inner: ShellTool,
}

impl BashTool {
    pub fn new(
        allowlist: Vec<String>,
        working_dir: Option<PathBuf>,
        requires_approval: bool,
    ) -> Self {
        Self {
            inner: ShellTool::new(allowlist, working_dir, requires_approval),
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn schema(&self) -> ToolSchema {
        let mut schema = self.inner.schema();
        schema.name = "bash".to_string();
        schema.description = if schema.requires_approval {
            "Execute a bash command. Requires approval. Only allowlisted commands are permitted."
        } else {
            "Execute a bash command. Only allowlisted commands are permitted."
        }
        .to_string();
        schema
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let command = args["command"].as_str().ok_or_else(|| {
            unly_core::Error::InvalidInput("missing command argument".to_string())
        })?;
        if !self.inner.is_allowed(command) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("command not in allowlist: {}", command),
                start.elapsed().as_millis() as u64,
            ));
        }
        let working_dir = self.inner.resolve_working_dir(&args);
        // Use a real bash invocation and pipefail for reliable exit behavior.
        let wrapped = format!("set -o pipefail; {}", command);
        Ok(self
            .inner
            .execute_with_program(
                "/usr/bin/env",
                &["bash", "-lc"],
                &wrapped,
                working_dir,
                ctx,
                start,
            )
            .await)
    }
}
