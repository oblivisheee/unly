use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

use unly_core::{
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
    Result,
};

/// Tool: Read a file.
pub struct FsReadTool;

#[async_trait]
impl Tool for FsReadTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_read".to_string(),
            description: "Read the contents of a file. Only allowed paths are accessible."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to read."
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Maximum number of bytes to read (default: 65536).",
                        "default": 65536
                    }
                },
                "required": ["path"]
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing path argument".to_string()))?;

        let max_bytes = args["max_bytes"].as_u64().unwrap_or(65536) as usize;

        let path = PathBuf::from(path_str);

        // Basic safety: no path traversal.
        if path_str.contains("..") {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "path traversal not allowed",
                start.elapsed().as_millis() as u64,
            ));
        }

        match std::fs::read(&path) {
            Ok(bytes) => {
                let content = if bytes.len() > max_bytes {
                    let truncated = &bytes[..max_bytes];
                    format!(
                        "{}\n[... truncated at {} bytes]",
                        String::from_utf8_lossy(truncated),
                        max_bytes
                    )
                } else {
                    String::from_utf8_lossy(&bytes).to_string()
                };
                Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    content,
                    start.elapsed().as_millis() as u64,
                ))
            }
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

/// Tool: List directory contents.
pub struct FsListTool;

#[async_trait]
impl Tool for FsListTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_list".to_string(),
            description: "List the contents of a directory.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the directory to list."
                    }
                },
                "required": ["path"]
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing path argument".to_string()))?;

        if path_str.contains("..") {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "path traversal not allowed",
                start.elapsed().as_millis() as u64,
            ));
        }

        let path = Path::new(path_str);
        match std::fs::read_dir(path) {
            Ok(entries) => {
                let mut lines = Vec::new();
                for entry in entries.flatten() {
                    let file_type = entry
                        .file_type()
                        .map(|ft| {
                            if ft.is_dir() {
                                "dir"
                            } else if ft.is_file() {
                                "file"
                            } else {
                                "other"
                            }
                        })
                        .unwrap_or("unknown");
                    let name = entry.file_name().to_string_lossy().to_string();
                    lines.push(format!("{}\t{}", file_type, name));
                }
                lines.sort();
                Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    lines.join("\n"),
                    start.elapsed().as_millis() as u64,
                ))
            }
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}
