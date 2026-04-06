//! Telegram output tools: send media files to the current Telegram chat.
//!
//! These tools do **not** communicate with the Telegram Bot API directly —
//! they validate the file exists on disk and embed the delivery instruction in
//! `ToolResult.metadata` under the `__telegram_send` key.  The agent runtime
//! detects that key and emits the corresponding `StreamEvent::SendMedia` event
//! (streaming path) or populates `AgentContext::pending_media` (non-streaming
//! path). The Telegram bot layer then calls the corresponding Telegram API
//! method (sendPhoto/sendDocument/sendVideo/sendAudio/sendVoice/sendAnimation).
//!
//! # Risk classification
//! Both tools are `Safe` — they do not mutate local state; they only queue an
//! outbound message.

use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use unly_core::{
    Result,
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
};

fn media_schema(name: &str, description: &str) -> ToolSchema {
    ToolSchema {
        name: name.to_string(),
        description: description.to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to send."
                },
                "caption": {
                    "type": "string",
                    "description": "Optional caption for the media (where Telegram supports captions)."
                }
            },
            "required": ["path"]
        }),
        risk: ToolRisk::Safe,
        requires_approval: false,
    }
}

fn queue_media_send(args: Value, ctx: &ToolContext, kind: &str, label: &str) -> Result<ToolResult> {
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
        format!("{label} queued for delivery to the Telegram chat: {path}"),
        start.elapsed().as_millis() as u64,
    );
    result.metadata = json!({
        "__telegram_send": {
            "kind": kind,
            "path": path,
            "caption": caption
        }
    });
    Ok(result)
}

pub struct TelegramSendPhotoTool;

#[async_trait]
impl Tool for TelegramSendPhotoTool {
    fn schema(&self) -> ToolSchema {
        media_schema(
            "telegram_send_photo",
            "Send an image file to the current Telegram chat using sendPhoto.",
        )
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        queue_media_send(args, ctx, "photo", "Photo")
    }
}

pub struct TelegramSendDocumentTool;

#[async_trait]
impl Tool for TelegramSendDocumentTool {
    fn schema(&self) -> ToolSchema {
        media_schema(
            "telegram_send_document",
            "Send a file to the current Telegram chat as a document using sendDocument.",
        )
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        queue_media_send(args, ctx, "document", "Document")
    }
}

pub struct TelegramSendVideoTool;

#[async_trait]
impl Tool for TelegramSendVideoTool {
    fn schema(&self) -> ToolSchema {
        media_schema(
            "telegram_send_video",
            "Send a video file to the current Telegram chat using sendVideo.",
        )
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        queue_media_send(args, ctx, "video", "Video")
    }
}

pub struct TelegramSendAudioTool;

#[async_trait]
impl Tool for TelegramSendAudioTool {
    fn schema(&self) -> ToolSchema {
        media_schema(
            "telegram_send_audio",
            "Send an audio file to the current Telegram chat using sendAudio.",
        )
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        queue_media_send(args, ctx, "audio", "Audio")
    }
}

pub struct TelegramSendVoiceTool;

#[async_trait]
impl Tool for TelegramSendVoiceTool {
    fn schema(&self) -> ToolSchema {
        media_schema(
            "telegram_send_voice",
            "Send a voice message file to the current Telegram chat using sendVoice.",
        )
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        queue_media_send(args, ctx, "voice", "Voice message")
    }
}

pub struct TelegramSendAnimationTool;

#[async_trait]
impl Tool for TelegramSendAnimationTool {
    fn schema(&self) -> ToolSchema {
        media_schema(
            "telegram_send_animation",
            "Send an animation (GIF/MP4) to the current Telegram chat using sendAnimation.",
        )
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        queue_media_send(args, ctx, "animation", "Animation")
    }
}
