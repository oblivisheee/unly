use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use std::sync::Arc;
use std::time::Duration;
use teloxide::{
    prelude::*,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message, ParseMode},
    utils::command::BotCommands,
};
use tokio::sync::mpsc;
use tracing::{error, info};

use unly_agent::{AgentContext, AgentResponse, AgentRuntime, StreamEvent};
use unly_audit::AuditLogger;
use unly_config::{workspace, AppConfig, DbType};
use unly_core::ids::ChatId;
use unly_db::Database;
use unly_providers::ProviderRegistry;

use crate::{
    commands::Command,
    permissions::{build_permissions, is_allowed},
    session::SessionStore,
};

const BOOT_DONE_PHRASES: [&str; 4] = ["done", "finish", "finished", "complete"];

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

        let handler = dptree::entry()
            .branch(
                Update::filter_message()
                    .branch(dptree::entry().filter_command::<Command>().endpoint({
                        let this = self.clone();
                        move |bot: Bot, msg: Message, cmd: Command| {
                            let this = this.clone();
                            async move { this.handle_command(bot, msg, cmd).await }
                        }
                    }))
                    .branch(dptree::endpoint({
                        let this = self.clone();
                        move |bot: Bot, msg: Message| {
                            let this = this.clone();
                            async move { this.handle_message(bot, msg).await }
                        }
                    })),
            )
            .branch(Update::filter_callback_query().endpoint({
                let this = self.clone();
                move |bot: Bot, q: CallbackQuery| {
                    let this = this.clone();
                    async move { this.handle_callback(bot, q).await }
                }
            }));

        let mut dispatcher = Dispatcher::builder(bot, handler).build();
        let shutdown_token = dispatcher.shutdown_token();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("ctrl+c received, starting graceful shutdown");
                let _ = shutdown_token.shutdown();
            }
        });

        dispatcher.dispatch().await;

        info!("telegram polling stopped");
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

        match cmd {
            Command::Start => {
                self.sessions.remove(tg_chat_id);
                if workspace::is_boot_mode() {
                    let boot_start = self.generate_boot_start_message(tg_user_id).await;
                    bot.send_message(msg.chat.id, boot_start).await?;
                    let _ = workspace::mark_boot_prompted();
                } else {
                    bot.send_message(msg.chat.id, "Hello. I'm Unly. How can I help?")
                        .await?;
                }
            }
            Command::Reset => {
                self.sessions.remove(tg_chat_id);
                bot.send_message(msg.chat.id, "Session reset. Send your next request.")
                    .await?;
            }

            Command::Help => {
                let text = Command::descriptions().to_string();
                bot.send_message(msg.chat.id, text).await?;
            }

            Command::Status => {
                let reports = self.provider_registry.health_all().await;
                let mut lines = vec!["System Status".to_string()];
                for r in &reports {
                    let icon = match r.status {
                        unly_core::types::HealthStatus::Healthy => "",
                        unly_core::types::HealthStatus::Degraded => "",
                        unly_core::types::HealthStatus::Unhealthy => "",
                        unly_core::types::HealthStatus::Unknown => "",
                    };
                    lines.push(format!(
                        "{} {}: {}",
                        icon,
                        r.name,
                        r.message.as_deref().unwrap_or("ok")
                    ));
                }
                let sessions = self.sessions.len();
                lines.push(format!(" Active sessions: {}", sessions));
                bot.send_message(msg.chat.id, lines.join("\n")).await?;
            }

            Command::Model(model_id) => {
                if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                    ctx.model = model_id.clone();
                    self.sessions.set(tg_chat_id, ctx);
                    bot.send_message(msg.chat.id, format!("Model set to {}", model_id))
                        .await?;
                } else {
                    bot.send_message(
                        msg.chat.id,
                        format!("Model {} will be used for the next conversation.", model_id),
                    )
                    .await?;
                }
            }

            Command::Provider(provider_name) => {
                if self.provider_registry.get(&provider_name).is_some() {
                    if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                        ctx.provider = provider_name.clone();
                        self.sessions.set(tg_chat_id, ctx);
                    }
                    bot.send_message(msg.chat.id, format!("Provider set to {}", provider_name))
                        .await?;
                } else {
                    let available = self.provider_registry.provider_names().join(", ");
                    bot.send_message(
                        msg.chat.id,
                        format!(
                            "Provider {} not found. Available: {}",
                            provider_name, available
                        ),
                    )
                    .await?;
                }
            }

            Command::Subagent => {
                let cfg = &self.config.agent;
                let text = format!(
                    "Subagents\n\n\
• Maximum depth: {}\n\
• Maximum concurrent subagents: {}\n\
• Token budget per subagent: {}\n\n\
Subagents are specialized execution contexts used for focused goals.",
                    cfg.max_subagent_depth, cfg.max_concurrent_subagents, cfg.subagent_token_budget
                );
                bot.send_message(msg.chat.id, text).await?;
            }
            Command::Subagents => {
                let cfg = &self.config.agent;
                let active = self.load_active_subagents().await;
                let mut text = format!(
                    "Subagents status\nMax depth: {}\nMax concurrent: {}\nToken budget: {}\n",
                    cfg.max_subagent_depth, cfg.max_concurrent_subagents, cfg.subagent_token_budget
                );
                if active.is_empty() {
                    text.push_str("\nActive: none");
                } else {
                    text.push_str("\nActive:\n");
                    for s in active {
                        text.push_str(&format!(
                            "- {} [{}] depth={} model={}\n",
                            s.id,
                            s.status,
                            s.depth,
                            s.model.unwrap_or_else(|| "default".to_string())
                        ));
                    }
                }
                bot.send_message(msg.chat.id, text).await?;
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
                            let names: Vec<&str> =
                                pending.iter().map(|p| p.tool_name.as_str()).collect();
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
                    bot.send_message(msg.chat.id, "ℹ No active session.")
                        .await?;
                }
            }

            Command::Deny => {
                if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                    let pending = std::mem::take(&mut ctx.pending_approvals);
                    self.sessions.set(tg_chat_id, ctx);
                    if pending.is_empty() {
                        bot.send_message(msg.chat.id, "ℹ No pending approvals.")
                            .await?;
                    } else {
                        let names: Vec<&str> =
                            pending.iter().map(|p| p.tool_name.as_str()).collect();
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
                    bot.send_message(msg.chat.id, "ℹ No active session.")
                        .await?;
                }
            }
            Command::Approval(mode) => match mode.trim().to_lowercase().as_str() {
                "auto" => {
                    self.sessions.set_auto_approve(tg_chat_id, true);
                    bot.send_message(
                            msg.chat.id,
                            "Approval mode set to AUTO. Pending tool actions will be approved automatically.",
                        )
                        .await?;
                }
                "manual" => {
                    self.sessions.set_auto_approve(tg_chat_id, false);
                    bot.send_message(
                        msg.chat.id,
                        "Approval mode set to MANUAL. Use /approve or /deny for pending actions.",
                    )
                    .await?;
                }
                _ => {
                    bot.send_message(
                        msg.chat.id,
                        "Invalid approval mode. Use: /approval manual or /approval auto",
                    )
                    .await?;
                }
            },
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

        if workspace::is_boot_mode() {
            if is_boot_done_signal(&text) {
                let summary = build_boot_summary();
                match workspace::finalize_boot(&summary) {
                    Ok(()) => {
                        bot.send_message(
                            msg.chat.id,
                            "BOOT completed.\n\nProfile processed and saved to MEMORY.md.\nBOOT.md was removed. Normal mode is now active.",
                        )
                        .await?;
                        return Ok(());
                    }
                    Err(e) => {
                        error!(chat_id = tg_chat_id, error = %e, "failed to finalize boot");
                        bot.send_message(
                            msg.chat.id,
                            " Failed to finalize BOOT profile. Please try again.",
                        )
                        .await?;
                        return Ok(());
                    }
                }
            }

            let note = format!(
                "\n## User Boot Input\n- chat: `{}`\n- text: {}\n",
                tg_chat_id,
                text.replace('\n', " ")
            );
            if let Err(e) = workspace::append_boot_notes(&note) {
                error!(chat_id = tg_chat_id, error = %e, "failed to append boot note");
            }
        }

        if workspace::is_boot_mode() && !workspace::is_boot_prompted() {
            let boot_hint = "BOOT mode is active\n\n\
I am currently in setup mode and can be tuned here.\n\
Primary memory root is MEMORY.md; linked memory/*.md files are additional AI-managed context.";
            if let Err(e) = bot.send_message(msg.chat.id, boot_hint).await {
                error!(chat_id = tg_chat_id, error = %e, "failed to send boot mode hint");
            } else {
                let _ = workspace::mark_boot_prompted();
            }
        }

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
                self.provider_registry
                    .default_provider()
                    .map(|p| p.name().to_string())
                    .unwrap_or_else(|_| "copilot".to_string()),
                self.provider_registry.default_model(),
                // system_prompt comes from runtime config; keep context-level value aligned.
                self.runtime.config().system_prompt.clone(),
            )
        });

        // Send a "typing..." indicator.
        bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
            .await?;

        // Persist the message.
        let chat_repo = unly_db::repo::chat::ChatRepo::new(self.db.conn());
        let chat_row = chat_repo
            .get_or_create_chat(tg_chat_id, msg.chat.title().or(msg.chat.username()))
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
            if let Err(e) = chat_repo.insert_message(&msg_row).await {
                error!(
                    chat_id = tg_chat_id,
                    user_id = tg_user_id,
                    error = %e,
                    "failed to persist user message"
                );
            }
        }

        // ── Streaming response ──────────────────────────────────────────────
        // Stream tokens and publish final response; use typing status as progress indicator.
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
        let runtime = self.runtime.clone();
        let sessions = self.sessions.clone();
        let mut ctx_clone = ctx.clone();

        let text_clone = text.clone();
        tokio::spawn(async move {
            if let Err(e) = runtime.process_stream(&mut ctx_clone, text_clone, tx).await {
                error!(chat_id = tg_chat_id, error = %e, "agent stream processing failed");
            }
            sessions.set(tg_chat_id, ctx_clone);
        });

        // Receive stream events and send final message chunks.
        let mut current_text = String::new();
        let mut last_typing = std::time::Instant::now();
        const TYPING_INTERVAL: Duration = Duration::from_secs(4);

        while let Some(event) = rx.recv().await {
            if last_typing.elapsed() >= TYPING_INTERVAL {
                let _ = bot
                    .send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
                    .await;
                last_typing = std::time::Instant::now();
            }
            match event {
                StreamEvent::ResponseStart => {
                    current_text.clear();
                }
                StreamEvent::Token(delta) => {
                    current_text.push_str(&delta);
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
                        if let Err(e) = chat_repo.insert_message(&msg_row).await {
                            error!(
                                chat_id = tg_chat_id,
                                user_id = tg_user_id,
                                error = %e,
                                "failed to persist assistant message"
                            );
                        }
                    }

                    if let Err(e) = send_response_text(&bot, msg.chat.id, &final_text).await {
                        error!(chat_id = tg_chat_id, error = %e, "failed to send final response");
                    }

                    self.audit
                        .success("agent_message", tg_user_id.to_string(), "process_message");
                    return Ok(());
                }
                StreamEvent::ApprovalRequired(pending) => {
                    if self.sessions.get_flags(tg_chat_id).auto_approve {
                        if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                            let response = self.runtime.process_approved(&mut ctx).await;
                            self.sessions.set(tg_chat_id, ctx);
                            match response {
                                Ok(AgentResponse::Text(text)) => {
                                    let _ = send_response_text(&bot, msg.chat.id, &text).await;
                                    return Ok(());
                                }
                                Ok(AgentResponse::ApprovalRequired { pending: _ }) => {
                                    // Fall through to manual request.
                                }
                                Err(e) => {
                                    bot.send_message(
                                        msg.chat.id,
                                        format!("Auto-approval failed: {}", e),
                                    )
                                    .await?;
                                    return Ok(());
                                }
                            }
                        }
                    }
                    let names: Vec<&str> = pending.iter().map(|p| p.tool_name.as_str()).collect();
                    let keyboard = InlineKeyboardMarkup::new(vec![vec![
                        InlineKeyboardButton::callback("Approve", "approve"),
                        InlineKeyboardButton::callback("Deny", "deny"),
                    ]]);
                    if let Err(e) = bot
                        .send_message(
                            msg.chat.id,
                            format!(
                                "The agent wants to use:\n{}\n\nDo you approve?",
                                names.join(", ")
                            ),
                        )
                        .reply_markup(keyboard)
                        .await
                    {
                        error!(chat_id = tg_chat_id, error = %e, "failed to send approval request");
                    }
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
        if let Err(e) = bot
            .send_message(
                msg.chat.id,
                " An error occurred while generating the response.",
            )
            .await
        {
            error!(chat_id = tg_chat_id, error = %e, "failed to send stream error message");
        }

        Ok(())
    }

    async fn load_active_subagents(&self) -> Vec<SubagentStatusRow> {
        let sql = "SELECT id, status, depth, model \
                   FROM subagents \
                   WHERE status IN ('pending','running') \
                   ORDER BY updated_at DESC \
                   LIMIT 10";
        let stmt = Statement::from_string(
            match self.db.db_type() {
                DbType::Postgres => DatabaseBackend::Postgres,
                DbType::Sqlite => DatabaseBackend::Sqlite,
            },
            sql.to_string(),
        );
        let rows = self.db.conn().query_all(stmt).await;
        match rows {
            Ok(rows) => rows
                .into_iter()
                .map(|r| SubagentStatusRow {
                    id: r
                        .try_get("", "id")
                        .unwrap_or_else(|_| "unknown".to_string()),
                    status: r
                        .try_get("", "status")
                        .unwrap_or_else(|_| "unknown".to_string()),
                    depth: r.try_get("", "depth").unwrap_or_default(),
                    model: r.try_get("", "model").ok(),
                })
                .collect(),
            Err(e) => {
                error!(error = %e, "failed to query subagents");
                Vec::new()
            }
        }
    }

    async fn handle_callback(
        &self,
        bot: Bot,
        q: CallbackQuery,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(data) = q.data.as_deref() else {
            return Ok(());
        };
        let Some(message) = q.message else {
            return Ok(());
        };
        let tg_chat_id = message.chat().id.0;
        let tg_user_id = q.from.id.0 as i64;

        if !is_allowed(
            tg_user_id,
            &self.config.telegram.admin_user_ids,
            &self.config.telegram.allowed_user_ids,
            self.config.telegram.open_access,
        ) {
            self.audit.denied(
                "telegram_callback_access",
                tg_user_id.to_string(),
                data.to_string(),
                "not in allowlist",
            );
            return Ok(());
        }

        let mut handled = false;
        match data {
            "approve" => {
                handled = true;
                if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                    if ctx.pending_approvals.is_empty() {
                        bot.send_message(message.chat().id, "No pending approvals.")
                            .await?;
                    } else {
                        let response = self.runtime.process_approved(&mut ctx).await;
                        self.sessions.set(tg_chat_id, ctx);
                        match response {
                            Ok(AgentResponse::Text(text)) => {
                                send_message_formatted(&bot, message.chat().id, text).await?;
                            }
                            Ok(AgentResponse::ApprovalRequired { pending }) => {
                                let names: Vec<&str> =
                                    pending.iter().map(|p| p.tool_name.as_str()).collect();
                                bot.send_message(
                                    message.chat().id,
                                    format!("Further approval required for: {}", names.join(", ")),
                                )
                                .await?;
                            }
                            Err(e) => {
                                bot.send_message(
                                    message.chat().id,
                                    format!("Error after approval: {}", e),
                                )
                                .await?;
                            }
                        }
                    }
                }
            }
            "deny" => {
                handled = true;
                if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                    let pending = std::mem::take(&mut ctx.pending_approvals);
                    self.sessions.set(tg_chat_id, ctx);
                    if pending.is_empty() {
                        bot.send_message(message.chat().id, "No pending approvals.")
                            .await?;
                    } else {
                        let names: Vec<&str> =
                            pending.iter().map(|p| p.tool_name.as_str()).collect();
                        bot.send_message(
                            message.chat().id,
                            format!("Denied tool executions: {}", names.join(", ")),
                        )
                        .await?;
                    }
                }
            }
            _ => {}
        }

        if handled {
            let _ = bot.answer_callback_query(q.id).await;
        }
        Ok(())
    }

    async fn generate_boot_start_message(&self, tg_user_id: i64) -> String {
        let permissions = build_permissions(tg_user_id, &self.config.telegram.admin_user_ids);
        let mut ctx = AgentContext::new(
            ChatId::new(),
            None,
            permissions,
            self.provider_registry
                .default_provider()
                .map(|p| p.name().to_string())
                .unwrap_or_else(|_| "copilot".to_string()),
            self.provider_registry.default_model(),
            self.runtime.config().system_prompt.clone(),
        );
        let prompt = "Generate one short onboarding message for first-time setup. Ask for user name, preferred communication style, and key constraints. End with: 'When you're finished, send \"done\" and I will save your configuration.' Return plain text only.";
        match self.runtime.process(&mut ctx, prompt).await {
            Ok(AgentResponse::Text(t)) if !t.trim().is_empty() => t,
            _ => "Hello! I'm Unly.\nSince this is our first conversation, please tell me your name, your preferred communication style, and any constraints I should follow.\n\nWhen you're finished, send \"done\" and I will save your configuration.".to_string(),
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Escape a string for use in Telegram HTML parse mode.
///
#[derive(Debug, Clone)]
struct SubagentStatusRow {
    id: String,
    status: String,
    depth: i32,
    model: Option<String>,
}

/// Split a message into chunks that fit within Telegram's limit.
fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(max_len).map(|c| c.iter().collect()).collect()
}

async fn send_response_text(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    text: &str,
) -> Result<(), teloxide::RequestError> {
    if text.is_empty() {
        return Ok(());
    }
    if text.len() <= 4000 {
        return send_message_formatted(bot, chat_id, text.to_string()).await;
    }

    // HTML parsing can fail when entities are split across chunks.
    // For long messages, send plain chunks for reliable delivery.
    for chunk in split_message(text, 4000) {
        if !chunk.is_empty() {
            bot.send_message(chat_id, chunk).await?;
        }
    }
    Ok(())
}

fn is_boot_done_signal(text: &str) -> bool {
    let normalized = text.trim().to_lowercase();
    BOOT_DONE_PHRASES.iter().any(|p| normalized == *p)
}

fn build_boot_summary() -> String {
    let boot_path = workspace::boot_path();
    let raw = std::fs::read_to_string(boot_path).unwrap_or_default();
    let mut lines = Vec::new();
    lines.push("- source: BOOT onboarding session".to_string());
    let extracted = raw
        .lines()
        .filter(|l| l.starts_with("- text:"))
        .take(20)
        .map(|l| format!("  {}", l))
        .collect::<Vec<_>>();
    if extracted.is_empty() {
        lines.push("- no explicit user profile lines captured".to_string());
    } else {
        lines.push("- captured profile notes:".to_string());
        lines.extend(extracted);
    }
    lines.join("\n")
}

async fn send_message_formatted(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    text: String,
) -> Result<(), teloxide::RequestError> {
    match bot
        .send_message(chat_id, text.clone())
        .parse_mode(ParseMode::Html)
        .await
    {
        Ok(_) => Ok(()),
        Err(_) => {
            bot.send_message(chat_id, text).await?;
            Ok(())
        }
    }
}
