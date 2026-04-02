use std::sync::Arc;
use std::time::Duration;
use teloxide::{
 prelude::*,
 types::{
 InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId, ParseMode,
 },
 utils::command::BotCommands,
};
use tokio::sync::mpsc;
use tracing::info;

use unly_agent::{AgentContext, AgentResponse, AgentRuntime, StreamEvent};
use unly_audit::AuditLogger;
use unly_config::AppConfig;
use unly_db::Database;
use unly_providers::ProviderRegistry;
use unly_core::ids::ChatId;

use crate::{
 commands::Command,
 permissions::{build_permissions, is_allowed, is_admin},
 session::SessionStore,
};

/// The main Telegram bot handler.
pub struct TelegramBot {
 config: Arc<AppConfig>,
 sessions: SessionStore,
 runtime: Arc<AgentRuntime>,
 provider_registry: Arc<ProviderRegistry>,
 db: Database,
 audit: Arc<AuditLogger>,
}

impl TelegramBot {
 pub fn new(
 config: Arc<AppConfig>,
 sessions: SessionStore,
 runtime: Arc<AgentRuntime>,
 provider_registry: Arc<ProviderRegistry>,
 db: Database,
 audit: Arc<AuditLogger>,
 ) -> Self {
 Self {
 config,
 sessions,
 runtime,
 provider_registry,
 db,
 audit,
 }
 }

 /// Start the Telegram bot polling loop.
 pub async fn start(self: Arc<Self>) {
 let token = self.config.telegram.bot_token.clone();
 let bot = Bot::new(token);

 info!("starting Telegram bot polling");

 let handler = Update::filter_message()
 .branch(
 dptree::entry()
 .filter_command::<Command>()
 .endpoint({
 let this = self.clone();
 move |bot: Bot, msg: Message, cmd: Command| {
 let this = this.clone();
 async move { this.handle_command(bot, msg, cmd).await }
 }
 }),
 )
 .branch(dptree::endpoint({
 let this = self.clone();
 move |bot: Bot, msg: Message| {
 let this = this.clone();
 async move { this.handle_message(bot, msg).await }
 }
 }));

 Dispatcher::builder(bot, handler)
 .enable_ctrlc_handler()
 .build()
 .dispatch()
 .await;
 }

 async fn handle_command(
 &self,
 bot: Bot,
 msg: Message,
 cmd: Command,
 ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
 let from = match msg.from {
 Some(ref u) => u.clone(),
 None => return Ok(()),
 };
 let tg_user_id = from.id.0 as i64;
 let tg_chat_id = msg.chat.id.0;

 // Access control.
 if !is_allowed(
 tg_user_id,
 &self.config.telegram.admin_user_ids,
 &self.config.telegram.allowed_user_ids,
 self.config.telegram.open_access,
 ) {
 self.audit.denied(
 "telegram_access",
 tg_user_id.to_string(),
 format!("command: {:?}", cmd),
 "not in allowlist",
 );
 bot.send_message(msg.chat.id, " You are not authorized to use this bot.")
 .await?;
 return Ok(());
 }

 let admin = is_admin(tg_user_id, &self.config.telegram.admin_user_ids);

 match cmd {
 Command::Start | Command::Reset => {
 self.sessions.remove(tg_chat_id);
 bot.send_message(
 msg.chat.id,
 " Hello! I'm <b>Unly</b>, your personal AI agent.\n\nSend me a message to get started, or use /help to see available commands.",
 )
 .parse_mode(ParseMode::Html)
 .await?;
 }

 Command::Help => {
 let text = Command::descriptions().to_string();
 bot.send_message(msg.chat.id, html_escape(&text))
 .parse_mode(ParseMode::Html)
 .await?;
 }

 Command::Status => {
 let reports = self.provider_registry.health_all().await;
 let mut lines = vec![" <b>System Status</b>".to_string()];
 for r in &reports {
 let icon = match r.status {
 unly_core::types::HealthStatus::Healthy => "",
 unly_core::types::HealthStatus::Degraded => "",
 unly_core::types::HealthStatus::Unhealthy => "",
 unly_core::types::HealthStatus::Unknown => "",
 };
 lines.push(format!(
 "{} <b>{}</b>: {}",
 icon,
 html_escape(&r.name),
 html_escape(r.message.as_deref().unwrap_or("ok"))
 ));
 }
 let sessions = self.sessions.len();
 lines.push(format!(" Active sessions: {}", sessions));
 bot.send_message(msg.chat.id, lines.join("\n"))
 .parse_mode(ParseMode::Html)
 .await?;
 }

 Command::Models => {
 let provider = self.provider_registry.default_provider();
 match provider {
 Ok(p) => match p.list_models().await {
 Ok(models) => {
 let mut lines =
 vec![format!(" <b>Models from {}:</b>", html_escape(p.name()))];
 for m in models.iter().take(20) {
 lines.push(format!(" • <code>{}</code>", html_escape(&m.id)));
 }
 bot.send_message(msg.chat.id, lines.join("\n"))
 .parse_mode(ParseMode::Html)
 .await?;
 }
 Err(e) => {
 bot.send_message(msg.chat.id, format!(" Failed to list models: {}", html_escape(&e.to_string())))
 .parse_mode(ParseMode::Html)
 .await?;
 }
 },
 Err(e) => {
 bot.send_message(msg.chat.id, format!(" No default provider: {}", html_escape(&e.to_string())))
 .parse_mode(ParseMode::Html)
 .await?;
 }
 }
 }

 Command::Model(model_id) => {
 if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
 ctx.model = model_id.clone();
 self.sessions.set(tg_chat_id, ctx);
 bot.send_message(
 msg.chat.id,
 format!(" Model set to <code>{}</code>", html_escape(&model_id)),
 )
 .parse_mode(ParseMode::Html)
 .await?;
 } else {
 bot.send_message(
 msg.chat.id,
 format!("Model <code>{}</code> will be used for the next conversation.", html_escape(&model_id)),
 )
 .parse_mode(ParseMode::Html)
 .await?;
 }
 }

 Command::Provider(provider_name) => {
 if self.provider_registry.get(&provider_name).is_some() {
 if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
 ctx.provider = provider_name.clone();
 self.sessions.set(tg_chat_id, ctx);
 }
 bot.send_message(
 msg.chat.id,
 format!(" Provider set to <code>{}</code>", html_escape(&provider_name)),
 )
 .parse_mode(ParseMode::Html)
 .await?;
 } else {
 let available = self.provider_registry.provider_names().join(", ");
 bot.send_message(
 msg.chat.id,
 format!(
 " Provider <code>{}</code> not found. Available: {}",
 html_escape(&provider_name), html_escape(&available)
 ),
 )
 .parse_mode(ParseMode::Html)
 .await?;
 }
 }

 Command::Approve => {
 if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
 if ctx.pending_approvals.is_empty() {
 bot.send_message(msg.chat.id, "ℹ No pending approvals.")
 .await?;
 return Ok(());
 }
 let approvals = ctx.pending_approvals.clone();
 let approval_names: Vec<&str> =
 approvals.iter().map(|a| a.tool_name.as_str()).collect();
 info!(
 user = tg_user_id,
 tools = ?approval_names,
 "user approved tool executions"
 );
 self.audit.success(
 "tool_approval",
 tg_user_id.to_string(),
 format!("approved: {}", approval_names.join(", ")),
 );

 let response = self.runtime.process_approved(&mut ctx).await;
 self.sessions.set(tg_chat_id, ctx);

 match response {
 Ok(AgentResponse::Text(text)) => {
 bot.send_message(msg.chat.id, text).await?;
 }
 Ok(AgentResponse::ApprovalRequired { pending }) => {
 let names: Vec<&str> = pending.iter().map(|p| p.tool_name.as_str()).collect();
 bot.send_message(
 msg.chat.id,
 format!(
 " Further approval required for: {}\n\nUse /approve to continue or /deny to cancel.",
 names.join(", ")
 ),
 )
 .await?;
 }
 Err(e) => {
 bot.send_message(msg.chat.id, format!(" Error after approval: {}", e))
 .await?;
 }
 }
 } else {
 bot.send_message(msg.chat.id, "ℹ No active session.").await?;
 }
 }

 Command::Deny => {
 if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
 let pending = std::mem::take(&mut ctx.pending_approvals);
 self.sessions.set(tg_chat_id, ctx);
 if pending.is_empty() {
 bot.send_message(msg.chat.id, "ℹ No pending approvals.").await?;
 } else {
 let names: Vec<&str> = pending.iter().map(|p| p.tool_name.as_str()).collect();
 self.audit.denied(
 "tool_approval",
 tg_user_id.to_string(),
 format!("denied: {}", names.join(", ")),
 "user denied",
 );
 bot.send_message(
 msg.chat.id,
 format!(" Denied tool executions: {}", names.join(", ")),
 )
 .await?;
 }
 } else {
 bot.send_message(msg.chat.id, "ℹ No active session.").await?;
 }
 }

 Command::Memory => {
 if !admin {
 bot.send_message(msg.chat.id, " Admin only.").await?;
 return Ok(());
 }
 bot.send_message(msg.chat.id, "ℹ Memory inspection via /memory is available for admin users.\n\nUse the CLI for detailed memory inspection: `unly memory list --scope chat:<id>`")
 .await?;
 }

 Command::Audit => {
 if !admin {
 bot.send_message(msg.chat.id, " Admin only.").await?;
 return Ok(());
 }
 let repo = unly_db::repo::audit::AuditRepo::new(self.db.conn());
 match repo.list_recent(10).await {
 Ok(rows) => {
 let mut lines = vec![" <b>Recent Audit Events</b>".to_string()];
 for row in &rows {
 lines.push(format!(
 "• {} <code>{}</code> {} → {}",
 row.created_at.format("%H:%M"),
 html_escape(&row.event_type),
 html_escape(&row.action),
 html_escape(&row.outcome)
 ));
 }
 bot.send_message(msg.chat.id, lines.join("\n"))
 .parse_mode(ParseMode::Html)
 .await?;
 }
 Err(e) => {
 bot.send_message(msg.chat.id, format!(" Audit log error: {}", html_escape(&e.to_string())))
 .parse_mode(ParseMode::Html)
 .await?;
 }
 }
 }

 Command::Jobs => {
 if !admin {
 bot.send_message(msg.chat.id, " Admin only.").await?;
 return Ok(());
 }
 let repo = unly_db::repo::job::JobRepo::new(self.db.conn());
 match repo.list_enabled().await {
 Ok(jobs) => {
 if jobs.is_empty() {
 bot.send_message(msg.chat.id, "ℹ No scheduled jobs configured.").await?;
 } else {
 let mut lines = vec![" <b>Scheduled Jobs</b>".to_string()];
 for job in &jobs {
 lines.push(format!(
 "• <b>{}</b> <code>{}</code> — last: {}",
 html_escape(&job.name),
 html_escape(job.cron_expression.as_deref().unwrap_or("adhoc")),
 job.last_run_at.map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "never".to_string())
 ));
 }
 bot.send_message(msg.chat.id, lines.join("\n"))
 .parse_mode(ParseMode::Html)
 .await?;
 }
 }
 Err(e) => {
 bot.send_message(msg.chat.id, format!(" Jobs error: {}", html_escape(&e.to_string())))
 .parse_mode(ParseMode::Html)
 .await?;
 }
 }
 }
 }

 Ok(())
 }

 async fn handle_message(
 &self,
 bot: Bot,
 msg: Message,
 ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
 let from = match msg.from {
 Some(ref u) => u.clone(),
 None => return Ok(()),
 };
 let tg_user_id = from.id.0 as i64;
 let tg_chat_id = msg.chat.id.0;

 // Access control.
 if !is_allowed(
 tg_user_id,
 &self.config.telegram.admin_user_ids,
 &self.config.telegram.allowed_user_ids,
 self.config.telegram.open_access,
 ) {
 bot.send_message(msg.chat.id, " You are not authorized to use this bot.")
 .await?;
 return Ok(());
 }

 let text = match msg.text() {
 Some(t) => t.to_string(),
 None => {
 // Handle file uploads gracefully.
 if msg.document().is_some() || msg.photo().is_some() {
 bot.send_message(
 msg.chat.id,
 " I received a file. File processing is not yet fully implemented in this version.",
 )
 .await?;
 }
 return Ok(());
 }
 };

 // Get or create session context.
  let ctx = self.sessions.get(tg_chat_id).unwrap_or_else(|| {
 let permissions = build_permissions(tg_user_id, &self.config.telegram.admin_user_ids);
 let chat_id = ChatId::new();
 // The runtime already has the system prompt baked in — pass empty here so
 // it gets overridden by the runtime's configured prompt on first build_messages().
 AgentContext::new(
 chat_id,
 None,
 permissions,
 self.provider_registry.default_provider()
 .map(|p| p.name().to_string())
 .unwrap_or_else(|_| "copilot".to_string()),
 self.provider_registry.default_model(),
 // system_prompt comes from IDENTITY.md / BOOT.md via the runtime config
 String::new(),
 )
 });

 // Send a "typing..." indicator.
 bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
 .await?;

 // Persist the message.
 let chat_repo = unly_db::repo::chat::ChatRepo::new(self.db.conn());
 let chat_row = chat_repo
 .get_or_create_chat(
 tg_chat_id,
 msg.chat.title().or(msg.chat.username()),
 )
 .await;

 if let Ok(chat_row) = &chat_row {
 let msg_row = unly_db::repo::chat::MessageRow {
 id: uuid::Uuid::new_v4().to_string(),
 chat_id: chat_row.id.clone(),
 user_id: None,
 role: "user".to_string(),
 content: serde_json::to_string(&serde_json::json!({"type": "text", "text": text}))
 .unwrap_or_default(),
 created_at: chrono::Utc::now(),
 metadata: "{}".to_string(),
 };
 let _ = chat_repo.insert_message(&msg_row).await;
 }

 // ── Streaming response ──────────────────────────────────────────────
 // 1. Send an initial placeholder message (" Thinking…")
 // 2. Process the agent in the background while updating the message
 // 3. When streaming is done, do a final edit with the full reply

 let placeholder = bot
 .send_message(msg.chat.id, " <i>Thinking…</i>")
 .parse_mode(ParseMode::Html)
 .await?;
 let placeholder_id: MessageId = placeholder.id;

 let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
 let runtime = self.runtime.clone();
 let sessions = self.sessions.clone();
 let mut ctx_clone = ctx.clone();

 let text_clone = text.clone();
 tokio::spawn(async move {
 let _ = runtime
 .process_stream(&mut ctx_clone, text_clone, tx)
 .await;
 sessions.set(tg_chat_id, ctx_clone);
 });

 // Receive stream events and update the Telegram message.
 let mut current_text = String::new();
 let mut last_edit = std::time::Instant::now();
 // Minimum interval between Telegram edits to avoid rate limiting.
 const EDIT_INTERVAL: Duration = Duration::from_millis(800);

 while let Some(event) = rx.recv().await {
 match event {
 StreamEvent::Thinking(tool_name) => {
 // Show which tool is being used in the thinking indicator.
 let thinking_text = format!(" <i>Thinking: {}</i>", html_escape(&tool_name));
 if last_edit.elapsed() >= EDIT_INTERVAL {
 let _ = bot
 .edit_message_text(msg.chat.id, placeholder_id, &thinking_text)
 .parse_mode(ParseMode::Html)
 .await;
 last_edit = std::time::Instant::now();
 }
 }
 StreamEvent::ResponseStart => {
 current_text.clear();
 }
 StreamEvent::Token(delta) => {
 current_text.push_str(&delta);
 // Throttle edits.
 if last_edit.elapsed() >= EDIT_INTERVAL && !current_text.trim().is_empty() {
 let display = truncate_for_telegram(&current_text) + " ▌";
 let _ = bot
 .edit_message_text(msg.chat.id, placeholder_id, &display)
 .parse_mode(ParseMode::Html)
 .await;
 last_edit = std::time::Instant::now();
 }
 }
 StreamEvent::Done(final_text) => {
 // Persist assistant message.
 if let Ok(chat_row) = &chat_row {
 let msg_row = unly_db::repo::chat::MessageRow {
 id: uuid::Uuid::new_v4().to_string(),
 chat_id: chat_row.id.clone(),
 user_id: None,
 role: "assistant".to_string(),
 content: serde_json::to_string(
 &serde_json::json!({"type": "text", "text": final_text}),
 )
 .unwrap_or_default(),
 created_at: chrono::Utc::now(),
 metadata: "{}".to_string(),
 };
 let _ = chat_repo.insert_message(&msg_row).await;
 }

 // Send the final response (split if too long).
 let chunks = split_message(&final_text, 4000);
 if chunks.is_empty() {
 let _ = bot
 .edit_message_text(msg.chat.id, placeholder_id, "")
 .await;
 } else {
 // Edit the placeholder with the first chunk.
 if !chunks[0].is_empty() {
 let _ = bot
 .edit_message_text(msg.chat.id, placeholder_id, &chunks[0])
 .parse_mode(ParseMode::Html)
 .await;
 }
 // Send remaining chunks as new messages.
 for chunk in chunks.iter().skip(1) {
 let _ = bot
 .send_message(msg.chat.id, chunk)
 .parse_mode(ParseMode::Html)
 .await;
 }
 }

 self.audit.success("agent_message", tg_user_id.to_string(), "process_message");
 return Ok(());
 }
 StreamEvent::ApprovalRequired(pending) => {
 let names: Vec<&str> = pending.iter().map(|p| p.tool_name.as_str()).collect();
 let keyboard = InlineKeyboardMarkup::new(vec![vec![
 InlineKeyboardButton::callback(" Approve", "approve"),
 InlineKeyboardButton::callback(" Deny", "deny"),
 ]]);
 let _ = bot
 .edit_message_text(
 msg.chat.id,
 placeholder_id,
 format!(
 " The agent wants to use: <b>{}</b>\n\nDo you approve?",
 html_escape(&names.join(", "))
 ),
 )
 .parse_mode(ParseMode::Html)
 .reply_markup(keyboard)
 .await;
 return Ok(());
 }
 }
 }

 // Channel closed without Done (error case).
 self.audit.failure(
 "agent_message",
 tg_user_id.to_string(),
 "process_message",
 "stream ended unexpectedly",
 );
 let _ = bot
 .edit_message_text(msg.chat.id, placeholder_id, " An error occurred while generating the response.")
 .await;

 Ok(())
 }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Escape a string for use in Telegram HTML parse mode.
///
/// Only the three characters that Telegram HTML requires escaping are handled:
/// `&` → `&amp;`, `<` → `&lt;`, `>` → `&gt;`.
pub fn html_escape(text: &str) -> String {
 text.replace('&', "&amp;")
 .replace('<', "&lt;")
 .replace('>', "&gt;")
}

/// Truncate text to fit within Telegram's 4096-character message limit,
/// preserving whole UTF-8 characters.
fn truncate_for_telegram(text: &str) -> String {
 const MAX: usize = 3900;
 if text.len() <= MAX {
 return text.to_string();
 }
 // Find the last char boundary at or before MAX.
 let mut end = MAX;
 while !text.is_char_boundary(end) {
 end -= 1;
 }
 format!("{}…", &text[..end])
}

/// Split a message into chunks that fit within Telegram's limit.
fn split_message(text: &str, max_len: usize) -> Vec<String> {
 if text.len() <= max_len {
 return vec![text.to_string()];
 }
 let chars: Vec<char> = text.chars().collect();
 chars
 .chunks(max_len)
 .map(|c| c.iter().collect())
 .collect()
}
