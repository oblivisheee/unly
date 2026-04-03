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

/// Tool: Write file contents.
pub struct FsWriteTool;

#[async_trait]
impl Tool for FsWriteTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_write".to_string(),
            description:
                "Write text to a file (overwrite by default, optional append). Creates parent directories if needed."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type":"string","description":"Absolute or relative file path."},
                    "content": {"type":"string","description":"Text content to write."},
                    "append": {"type":"boolean","description":"Append instead of overwrite.","default": false}
                },
                "required": ["path", "content"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing path argument".to_string()))?;
        let content = args["content"].as_str().ok_or_else(|| {
            unly_core::Error::InvalidInput("missing content argument".to_string())
        })?;
        let append = args["append"].as_bool().unwrap_or(false);

        if path_str.contains("..") {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "path traversal not allowed",
                start.elapsed().as_millis() as u64,
            ));
        }

        let path = PathBuf::from(path_str);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e.to_string(),
                    start.elapsed().as_millis() as u64,
                ));
            }
        }

        let result = if append {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, content.as_bytes()))
        } else {
            std::fs::write(&path, content.as_bytes())
        };

        match result {
            Ok(_) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("written: {}", path.display()),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Additional file tools
// ──────────────────────────────────────────────────────────────────────────────

/// Default maximum number of lines returned by fs_grep.
const DEFAULT_MAX_GREP_RESULTS: usize = 100;

/// Shared path validation helper: rejects `..` traversal and returns a
/// `PathBuf` on success.
///
/// A `..` component check is the primary guard for all file tools.  This
/// prevents the most common path-traversal attack vectors at the argument
/// level before any I/O occurs.  Symlink resolution is the responsibility of
/// the OS; callers that need strict base-directory confinement should apply
/// an additional `canonicalize`-based check after receiving the PathBuf.
fn validate_path(path_str: &str) -> std::result::Result<PathBuf, &'static str> {
    // Reject any literal `..` path component as a traversal attempt.
    // This covers the most common attack patterns including adjacent slashes.
    if path_str
        .split(['/', '\\'])
        .any(|component| component == "..")
    {
        return Err("path traversal not allowed");
    }
    Ok(PathBuf::from(path_str))
}

// ── fs_delete ────────────────────────────────────────────────────────────────

/// Tool: Delete a file or directory.
pub struct FsDeleteTool;

#[async_trait]
impl Tool for FsDeleteTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_delete".to_string(),
            description: "Delete a file or directory. Directories are removed recursively."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the file or directory to delete."},
                    "recursive": {"type": "boolean", "description": "Remove directory and all contents recursively (default: false)."}
                },
                "required": ["path"]
            }),
            risk: ToolRisk::Dangerous,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing path argument".to_string()))?;
        let path = match validate_path(path_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };
        let recursive = args["recursive"].as_bool().unwrap_or(false);

        // Use symlink_metadata so we correctly handle symlinks and special
        // files: is_dir() follows symlinks (could be wrong for a dangling
        // symlink), while symlink_metadata().is_dir() does not.
        let result = match std::fs::symlink_metadata(&path) {
            Err(e) => Err(e),
            Ok(meta) => {
                if meta.is_dir() {
                    if recursive {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_dir(&path)
                    }
                } else {
                    // Covers regular files, symlinks, and other special files.
                    std::fs::remove_file(&path)
                }
            }
        };

        match result {
            Ok(_) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("deleted: {}", path.display()),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── fs_copy ──────────────────────────────────────────────────────────────────

/// Tool: Copy a file.
pub struct FsCopyTool;

#[async_trait]
impl Tool for FsCopyTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_copy".to_string(),
            description: "Copy a file from one path to another.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "src": {"type": "string", "description": "Source file path."},
                    "dst": {"type": "string", "description": "Destination file path."}
                },
                "required": ["src", "dst"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let src_str = args["src"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing src argument".to_string()))?;
        let dst_str = args["dst"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing dst argument".to_string()))?;

        let src = match validate_path(src_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };
        let dst = match validate_path(dst_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };

        if let Some(parent) = dst.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    format!("failed to create destination directory: {}", e),
                    start.elapsed().as_millis() as u64,
                ));
            }
        }

        match std::fs::copy(&src, &dst) {
            Ok(bytes) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("copied {} bytes: {} -> {}", bytes, src.display(), dst.display()),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── fs_move ──────────────────────────────────────────────────────────────────

/// Tool: Move (rename) a file or directory.
pub struct FsMoveTool;

#[async_trait]
impl Tool for FsMoveTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_move".to_string(),
            description: "Move or rename a file or directory.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "src": {"type": "string", "description": "Source path."},
                    "dst": {"type": "string", "description": "Destination path."}
                },
                "required": ["src", "dst"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let src_str = args["src"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing src argument".to_string()))?;
        let dst_str = args["dst"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing dst argument".to_string()))?;

        let src = match validate_path(src_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };
        let dst = match validate_path(dst_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };

        if let Some(parent) = dst.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    format!("failed to create destination directory: {}", e),
                    start.elapsed().as_millis() as u64,
                ));
            }
        }

        match std::fs::rename(&src, &dst) {
            Ok(_) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("moved: {} -> {}", src.display(), dst.display()),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── fs_mkdir ─────────────────────────────────────────────────────────────────

/// Tool: Create a directory (and all missing parent directories).
pub struct FsMkdirTool;

#[async_trait]
impl Tool for FsMkdirTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_mkdir".to_string(),
            description: "Create a directory (and all missing parent directories).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory path to create."}
                },
                "required": ["path"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing path argument".to_string()))?;
        let path = match validate_path(path_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };

        match std::fs::create_dir_all(&path) {
            Ok(_) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("created: {}", path.display()),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── fs_stat ──────────────────────────────────────────────────────────────────

/// Tool: Get metadata/stat for a file or directory.
pub struct FsStatTool;

#[async_trait]
impl Tool for FsStatTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_stat".to_string(),
            description: "Get metadata for a file or directory (size, type, modified time, permissions).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to inspect."}
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
        let path = match validate_path(path_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };

        match std::fs::metadata(&path) {
            Ok(meta) => {
                let kind = if meta.is_dir() {
                    "directory"
                } else if meta.is_file() {
                    "file"
                } else {
                    "other"
                };
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_secs())
                    })
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                #[cfg(unix)]
                let permissions = {
                    use std::os::unix::fs::PermissionsExt;
                    format!("{:o}", meta.permissions().mode())
                };
                #[cfg(not(unix))]
                let permissions = if meta.permissions().readonly() {
                    "readonly".to_string()
                } else {
                    "readwrite".to_string()
                };
                let result = serde_json::json!({
                    "path": path_str,
                    "type": kind,
                    "size_bytes": meta.len(),
                    "readonly": meta.permissions().readonly(),
                    "permissions": permissions,
                    "modified_unix": modified,
                });
                Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    result.to_string(),
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

// ── fs_grep ──────────────────────────────────────────────────────────────────

/// Tool: Search for a pattern in files (grep).
pub struct FsGrepTool;

#[async_trait]
impl Tool for FsGrepTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fs_grep".to_string(),
            description: "Search for a text pattern in files under a directory. Returns matching lines with file paths and line numbers.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Text pattern to search for."},
                    "path": {"type": "string", "description": "File or directory to search in."},
                    "case_insensitive": {"type": "boolean", "description": "Case-insensitive search (default: false)."},
                    "max_results": {"type": "integer", "description": "Maximum number of matching lines to return (default: 100)."}
                },
                "required": ["pattern", "path"]
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing pattern argument".to_string()))?;
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing path argument".to_string()))?;
        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
        let max_results = args["max_results"].as_u64().unwrap_or(DEFAULT_MAX_GREP_RESULTS as u64) as usize;

        let path = match validate_path(path_str) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    e,
                    start.elapsed().as_millis() as u64,
                ))
            }
        };

        let mut matches: Vec<String> = Vec::new();
        grep_path(&path, pattern, case_insensitive, max_results, &mut matches);

        if matches.is_empty() {
            Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                "no matches found".to_string(),
                start.elapsed().as_millis() as u64,
            ))
        } else {
            Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                matches.join("\n"),
                start.elapsed().as_millis() as u64,
            ))
        }
    }
}

/// Recursively grep a path for a pattern, appending results to `out`.
fn grep_path(path: &Path, pattern: &str, case_insensitive: bool, max: usize, out: &mut Vec<String>) {
    if out.len() >= max {
        return;
    }
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if out.len() >= max {
                    break;
                }
                grep_path(&entry.path(), pattern, case_insensitive, max, out);
            }
        }
    } else if path.is_file() {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return, // skip binary / unreadable files
        };
        let pat_lower = if case_insensitive {
            pattern.to_lowercase()
        } else {
            pattern.to_string()
        };
        for (line_num, line) in content.lines().enumerate() {
            if out.len() >= max {
                break;
            }
            let haystack = if case_insensitive {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            if haystack.contains(pat_lower.as_str()) {
                out.push(format!("{}:{}: {}", path.display(), line_num + 1, line));
            }
        }
    }
}
