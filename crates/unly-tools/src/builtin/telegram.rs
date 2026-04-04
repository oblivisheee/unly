//! Telegram output tools: send a photo or a file to the current Telegram chat.
//!
//! These tools do **not** communicate with the Telegram Bot API directly —
//! they validate the file exists on disk and embed the delivery instruction in
//! `ToolResult.metadata` under the `__telegram_send` key.  The agent runtime
//! detects that key and emits the corresponding `StreamEvent::SendMedia` event
//! (streaming path) or populates `AgentContext::pending_media` (non-streaming
//! path).  The Telegram bot layer then calls `send_photo` / `send_document`.
//!
//! # Supported file types for `telegram_send_photo`
//! JPEG, PNG, GIF, BMP, WEBP — anything Telegram accepts as a photo upload.
//! For other file types use `telegram_send_document`.
//!
//! # Risk classification
//! Both tools are `Safe` — they do not mutate local state; they only queue an
//! outbound message.

use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};

use unly_core::{
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
    Result,
};

// ── telegram_send_photo ───────────────────────────────────────────────────────

/// Send a photo to the current Telegram chat.
pub struct TelegramSendPhotoTool;

#[async_trait]
impl Tool for TelegramSendPhotoTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "telegram_send_photo".to_string(),
            description: "Send an image file to the current Telegram chat. \
                Supports JPEG, PNG, GIF, BMP, WEBP. \
                The file must already exist on disk. \
                Use `telegram_send_document` for non-image files."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the image file to send."
                    },
                    "caption": {
                        "type": "string",
                        "description": "Optional caption for the photo (max 1024 characters)."
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
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'path'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };

        if !std::path::Path::new(&path).exists() {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("file not found: {}", path),
                start.elapsed().as_millis() as u64,
            ));
        }

        let caption = args
            .get("caption")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut result = ToolResult::success(
            ctx.tool_call_id.clone(),
            format!("Photo queued for delivery to the Telegram chat: {}", path),
            start.elapsed().as_millis() as u64,
        );
        result.metadata = json!({
            "__telegram_send": {
                "kind": "photo",
                "path": path,
                "caption": caption
            }
        });
        Ok(result)
    }
}

// ── telegram_send_document ────────────────────────────────────────────────────

/// Send any file to the current Telegram chat as a document attachment.
pub struct TelegramSendDocumentTool;

#[async_trait]
impl Tool for TelegramSendDocumentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "telegram_send_document".to_string(),
            description: "Send any file to the current Telegram chat as a document attachment. \
                Use this for PDFs, ZIPs, text files, code files, or any non-image file. \
                For images (JPEG/PNG/GIF/WEBP) prefer `telegram_send_photo`. \
                The file must already exist on disk."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the file to send."
                    },
                    "caption": {
                        "type": "string",
                        "description": "Optional caption for the document (max 1024 characters)."
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
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'path'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };

        if !std::path::Path::new(&path).exists() {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("file not found: {}", path),
                start.elapsed().as_millis() as u64,
            ));
        }

        let caption = args
            .get("caption")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut result = ToolResult::success(
            ctx.tool_call_id.clone(),
            format!(
                "Document queued for delivery to the Telegram chat: {}",
                path
            ),
            start.elapsed().as_millis() as u64,
        );
        result.metadata = json!({
            "__telegram_send": {
                "kind": "document",
                "path": path,
                "caption": caption
            }
        });
        Ok(result)
    }
}
