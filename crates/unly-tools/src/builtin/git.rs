use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use unly_core::{
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
    Result,
};

/// Tool: git status.
pub struct GitStatusTool;

fn run_git(args: &[&str], working_dir: Option<&PathBuf>) -> std::result::Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    cmd.args(args);
    match cmd.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                Ok(stdout)
            } else {
                Err(format!("{}\n{}", stdout, stderr))
            }
        }
        Err(e) => Err(format!("failed to run git: {}", e)),
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "git_status".to_string(),
            description: "Run `git status` in a repository directory.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the git repository. Defaults to current directory."
                    }
                }
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let dir = args["path"].as_str().map(PathBuf::from);
        match run_git(&["status", "--short"], dir.as_ref()) {
            Ok(output) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                output,
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

/// Tool: git log.
pub struct GitLogTool;

#[async_trait]
impl Tool for GitLogTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "git_log".to_string(),
            description: "Show recent git log entries.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the git repository."
                    },
                    "n": {
                        "type": "integer",
                        "description": "Number of commits to show (default: 10).",
                        "default": 10
                    }
                }
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let dir = args["path"].as_str().map(PathBuf::from);
        let n = args["n"].as_u64().unwrap_or(10).min(100).to_string();
        match run_git(&["log", "--oneline", &format!("-{}", n)], dir.as_ref()) {
            Ok(output) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                output,
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}
