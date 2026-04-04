use async_trait::async_trait;
use chrono::Utc;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Instant;
use tokio::process::Command;
use uuid::Uuid;

use unly_core::{
    Result,
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
};

/// Tool: execute a shell command.
///
/// IMPORTANT: This tool is Dangerous and always requires approval.
/// The command must match the configured shell_allowlist policy.
/// Executed via bash -lc with restricted environment.
pub struct ShellTool {
    allowlist: Vec<String>,
    working_dir: Option<PathBuf>,
    requires_approval: bool,
}

#[derive(Debug, Clone)]
struct BashJobStatus {
    status: String,
    command: String,
    started_at: String,
    finished_at: Option<String>,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

static BASH_JOBS: Lazy<Mutex<HashMap<String, BashJobStatus>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

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
            if let Ok(re) = regex::Regex::new(pattern)
                && re.is_match(command)
            {
                return true;
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

    fn wrap_with_bashrc(command: &str) -> String {
        format!(
            "if [ -f ~/.bashrc ]; then source ~/.bashrc; fi; {}",
            command
        )
    }

    fn wrap_bash_command(command: &str) -> String {
        format!(
            "if [ -f ~/.bashrc ]; then source ~/.bashrc; fi; set -o pipefail; {}",
            command
        )
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
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // Restricted environment: only essential vars.
        // Using login-shell semantics lets shell startup files populate PATH.
        cmd.env_clear();
        cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin");
        cmd.env("CI", "1");
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
                    },
                    "mode": {
                        "type": "string",
                        "description": "Execution mode: run (default), start (background), or status.",
                        "enum": ["run", "start", "status"]
                    },
                    "job_id": {
                        "type": "string",
                        "description": "Required for mode=status. Job id returned by mode=start."
                    }
                },
                "required": []
            }),
            risk: ToolRisk::Dangerous,
            requires_approval: self.requires_approval,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let approved = args
            .get("__approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mode = args["mode"].as_str().unwrap_or("run");
        if mode == "status" {
            let job_id = args["job_id"].as_str().ok_or_else(|| {
                unly_core::Error::InvalidInput(
                    "missing job_id argument for status mode".to_string(),
                )
            })?;
            let jobs = BASH_JOBS.lock().map_err(|_| {
                unly_core::Error::Agent("bash job status lock poisoned".to_string())
            })?;
            if let Some(job) = jobs.get(job_id) {
                return Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    serde_json::to_string(&json!({
                        "job_id": job_id,
                        "status": job.status,
                        "command": job.command,
                        "started_at": job.started_at,
                        "finished_at": job.finished_at,
                        "exit_code": job.exit_code,
                        "stdout": job.stdout,
                        "stderr": job.stderr
                    }))
                    .unwrap_or_else(|_| "{}".to_string()),
                    start.elapsed().as_millis() as u64,
                ));
            }
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("job not found: {}", job_id),
                start.elapsed().as_millis() as u64,
            ));
        }

        let command = args["command"].as_str().ok_or_else(|| {
            unly_core::Error::InvalidInput("missing command argument".to_string())
        })?;

        // Policy: check allowlist.
        if !approved && !self.is_allowed(command) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("command not in allowlist: {}", command),
                start.elapsed().as_millis() as u64,
            ));
        }

        let working_dir = self.resolve_working_dir(&args);
        if mode == "start" {
            let job_id = Uuid::new_v4().to_string();
            {
                let mut jobs = BASH_JOBS.lock().map_err(|_| {
                    unly_core::Error::Agent("bash job state lock poisoned".to_string())
                })?;
                jobs.insert(
                    job_id.clone(),
                    BashJobStatus {
                        status: "running".to_string(),
                        command: command.to_string(),
                        started_at: Utc::now().to_rfc3339(),
                        finished_at: None,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                    },
                );
            }
            let program = "/usr/bin/env".to_string();
            let program_args = vec!["bash".to_string(), "-lc".to_string()];
            let command = Self::wrap_with_bashrc(command);
            let job_id_for_task = job_id.clone();
            tokio::spawn(async move {
                let mut cmd = Command::new(program);
                cmd.args(program_args).arg(command.clone());
                cmd.stdin(Stdio::null());
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());
                cmd.env_clear();
                cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin");
                cmd.env("CI", "1");
                if let Ok(home) = std::env::var("HOME") {
                    cmd.env("HOME", home);
                }
                if let Some(dir) = working_dir {
                    cmd.current_dir(dir);
                }
                let finished_at = Utc::now().to_rfc3339();
                match cmd.output().await {
                    Ok(output) => {
                        if let Ok(mut jobs) = BASH_JOBS.lock()
                            && let Some(job) = jobs.get_mut(&job_id_for_task)
                        {
                            job.status = if output.status.success() {
                                "completed".to_string()
                            } else {
                                "failed".to_string()
                            };
                            job.finished_at = Some(finished_at);
                            job.exit_code = output.status.code();
                            job.stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            job.stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        }
                    }
                    Err(e) => {
                        if let Ok(mut jobs) = BASH_JOBS.lock()
                            && let Some(job) = jobs.get_mut(&job_id_for_task)
                        {
                            job.status = "failed".to_string();
                            job.finished_at = Some(finished_at);
                            job.exit_code = Some(1);
                            job.stderr = e.to_string();
                        }
                    }
                }
            });
            return Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!(
                    "Bash started in background.\njob_id={}\nUse mode=status with this job_id to query progress.",
                    job_id
                ),
                start.elapsed().as_millis() as u64,
            ));
        }
        let wrapped = Self::wrap_with_bashrc(command);
        Ok(self
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
        let approved = args
            .get("__approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mode = args["mode"].as_str().unwrap_or("run");
        if mode == "status" {
            let mut shell_args = args.clone();
            if shell_args.get("job_id").is_none() {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing job_id argument for status mode".to_string(),
                    start.elapsed().as_millis() as u64,
                ));
            }
            shell_args["mode"] = Value::String("status".to_string());
            return self.inner.execute(shell_args, ctx).await;
        }
        let command = args["command"].as_str().ok_or_else(|| {
            unly_core::Error::InvalidInput("missing command argument".to_string())
        })?;
        if !approved && !self.inner.is_allowed(command) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("command not in allowlist: {}", command),
                start.elapsed().as_millis() as u64,
            ));
        }
        let working_dir = self.inner.resolve_working_dir(&args);
        if mode == "start" {
            let job_id = Uuid::new_v4().to_string();
            {
                let mut jobs = BASH_JOBS.lock().map_err(|_| {
                    unly_core::Error::Agent("bash job state lock poisoned".to_string())
                })?;
                jobs.insert(
                    job_id.clone(),
                    BashJobStatus {
                        status: "running".to_string(),
                        command: command.to_string(),
                        started_at: Utc::now().to_rfc3339(),
                        finished_at: None,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                    },
                );
            }
            let wrapped = ShellTool::wrap_bash_command(command);
            let job_id_for_task = job_id.clone();
            tokio::spawn(async move {
                let mut cmd = Command::new("/usr/bin/env");
                cmd.args(["bash", "-lc"]).arg(wrapped);
                cmd.stdin(Stdio::null());
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());
                cmd.env_clear();
                cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin");
                cmd.env("CI", "1");
                if let Ok(home) = std::env::var("HOME") {
                    cmd.env("HOME", home);
                }
                if let Some(dir) = working_dir {
                    cmd.current_dir(dir);
                }
                let finished_at = Utc::now().to_rfc3339();
                match cmd.output().await {
                    Ok(output) => {
                        if let Ok(mut jobs) = BASH_JOBS.lock()
                            && let Some(job) = jobs.get_mut(&job_id_for_task)
                        {
                            job.status = if output.status.success() {
                                "completed".to_string()
                            } else {
                                "failed".to_string()
                            };
                            job.finished_at = Some(finished_at);
                            job.exit_code = output.status.code();
                            job.stdout = String::from_utf8_lossy(&output.stdout).to_string();
                            job.stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        }
                    }
                    Err(e) => {
                        if let Ok(mut jobs) = BASH_JOBS.lock()
                            && let Some(job) = jobs.get_mut(&job_id_for_task)
                        {
                            job.status = "failed".to_string();
                            job.finished_at = Some(finished_at);
                            job.exit_code = Some(1);
                            job.stderr = e.to_string();
                        }
                    }
                }
            });
            return Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!(
                    "Background bash job started.\njob_id={}\nUse mode=status with this job_id to query progress.",
                    job_id
                ),
                start.elapsed().as_millis() as u64,
            ));
        }
        // Use a real bash invocation and pipefail for reliable exit behavior.
        let wrapped = ShellTool::wrap_bash_command(command);
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

#[cfg(test)]
mod tests {
    use super::ShellTool;

    #[test]
    fn wraps_shell_command_with_bashrc_source() {
        let wrapped = ShellTool::wrap_with_bashrc("echo ok");
        assert!(wrapped.starts_with("if [ -f ~/.bashrc ]; then source ~/.bashrc; fi;"));
        assert!(wrapped.ends_with("echo ok"));
    }

    #[test]
    fn wraps_bash_command_with_bashrc_and_pipefail() {
        let wrapped = ShellTool::wrap_bash_command("echo ok");
        assert!(wrapped.contains("source ~/.bashrc; fi; set -o pipefail;"));
        assert!(wrapped.ends_with("echo ok"));
    }
}
