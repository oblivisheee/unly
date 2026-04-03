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
                    send_response_text(&bot, msg.chat.id, &boot_start).await?;
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
                            send_response_text(&bot, msg.chat.id, &text).await?;
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
                                send_response_text(&bot, message.chat().id, &text).await?;
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
        let prompt = "Generate a warm, friendly first-time welcome message (3-5 sentences). \
Introduce yourself as Unly, mention this is a one-time personalisation setup, \
and invite the user to share: their name or preferred address, communication style (concise/detailed, formal/casual), \
and key areas they want help with. \
End with exactly: 'When you are done, just type done.' \
Return plain text only, no markdown formatting.";
        match self.runtime.process(&mut ctx, prompt).await {
            Ok(AgentResponse::Text(t)) if !t.trim().is_empty() => t,
            _ => "Hi! I'm Unly, your personal AI agent.\n\n\
Since this is our first conversation, I'd love to get to know you a little so I can serve you better.\n\n\
Could you tell me:\n\
• Your name or how you'd like me to address you\n\
• Your preferred communication style (brief and direct, or detailed explanations)\n\
• What you mainly want to use me for\n\n\
When you are done, just type done."
                .to_string(),
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

// ── message formatting helpers ────────────────────────────────────────────────

/// Convert a raw LLM response (which may contain Markdown, existing Telegram
/// HTML tags, or plain text) to well-formed Telegram HTML.
///
/// Handles, in priority order:
///  - Fenced code blocks (```…```) → `<pre>…</pre>`
///  - Inline code (`…`) → `<code>…</code>`
///  - Existing Telegram HTML tags (pass through unchanged)
///  - Bold **…** → `<b>…</b>`
///  - Strikethrough ~~…~~ → `<s>…</s>`
///  - Italic *…* → `<i>…</i>`
///  - Markdown links [text](url) → `<a href="url">text</a>`
///  - HTML-escaping of literal `&`, `<`, `>` in plain text
fn convert_to_telegram_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 256);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // ── Fenced code block ``` … ``` ──────────────────────────────────────
        if i + 2 < n && chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            i += 3;
            // skip optional language hint (everything up to the first newline)
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            if i < n {
                i += 1; // consume the newline
            }
            let code_start = i;
            let mut closed = false;
            while i + 2 < n {
                if chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
                    let code: String = chars[code_start..i].iter().collect();
                    let code = code.trim_end_matches('\n');
                    out.push_str("<pre>");
                    push_html_escaped(&mut out, code);
                    out.push_str("</pre>");
                    i += 3;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                // unclosed fence – output raw escaped content
                let code: String = chars[code_start..i].iter().collect();
                push_html_escaped(&mut out, &code);
            }
            continue;
        }

        // ── Inline code ` … ` ────────────────────────────────────────────────
        if chars[i] == '`' {
            let start = i + 1;
            if let Some(j) = chars[start..].iter().position(|&c| c == '`') {
                let code: String = chars[start..start + j].iter().collect();
                out.push_str("<code>");
                push_html_escaped(&mut out, &code);
                out.push_str("</code>");
                i = start + j + 1;
                continue;
            }
        }

        // ── Existing Telegram HTML tag – pass through ─────────────────────────
        if chars[i] == '<' {
            // Search for the closing '>' directly in the chars slice.
            if let Some(tag_len) = chars[i..].iter().position(|&c| c == '>') {
                // Build only the candidate tag string (typically very short).
                let candidate: String = chars[i..=i + tag_len].iter().collect();
                if is_telegram_html_tag(&candidate) {
                    out.push_str(&candidate);
                    i += tag_len + 1;
                    continue;
                }
            }
            // Not a known Telegram tag – escape the '<'
            out.push_str("&lt;");
            i += 1;
            continue;
        }

        // ── Bold **…** ────────────────────────────────────────────────────────
        if i + 1 < n && chars[i] == '*' && chars[i + 1] == '*' {
            let start = i + 2;
            let inner = &chars[start..];
            if let Some(j) = inner.windows(2).position(|w| w[0] == '*' && w[1] == '*') {
                let txt: String = inner[..j].iter().collect();
                out.push_str("<b>");
                push_html_escaped(&mut out, &txt);
                out.push_str("</b>");
                i = start + j + 2;
                continue;
            }
        }

        // ── Strikethrough ~~…~~ ───────────────────────────────────────────────
        if i + 1 < n && chars[i] == '~' && chars[i + 1] == '~' {
            let start = i + 2;
            let inner = &chars[start..];
            if let Some(j) = inner.windows(2).position(|w| w[0] == '~' && w[1] == '~') {
                let txt: String = inner[..j].iter().collect();
                out.push_str("<s>");
                push_html_escaped(&mut out, &txt);
                out.push_str("</s>");
                i = start + j + 2;
                continue;
            }
        }

        // ── Italic *…* ────────────────────────────────────────────────────────
        if chars[i] == '*' && (i + 1 >= n || chars[i + 1] != '*') {
            let start = i + 1;
            if let Some(j) = chars[start..].iter().position(|&c| c == '*') {
                if j > 0 {
                    let txt: String = chars[start..start + j].iter().collect();
                    out.push_str("<i>");
                    push_html_escaped(&mut out, &txt);
                    out.push_str("</i>");
                    i = start + j + 1;
                    continue;
                }
            }
        }

        // ── Markdown link [text](url) ─────────────────────────────────────────
        if chars[i] == '[' {
            let text_start = i + 1;
            if let Some(bracket_close) = chars[text_start..].iter().position(|&c| c == ']') {
                let after = text_start + bracket_close + 1;
                if after < n && chars[after] == '(' {
                    let url_start = after + 1;
                    if let Some(paren_close) = chars[url_start..].iter().position(|&c| c == ')') {
                        let link_text: String = chars[text_start..text_start + bracket_close]
                            .iter()
                            .collect();
                        let url: String =
                            chars[url_start..url_start + paren_close].iter().collect();
                        out.push_str("<a href=\"");
                        push_html_escaped(&mut out, &url);
                        out.push_str("\">");
                        push_html_escaped(&mut out, &link_text);
                        out.push_str("</a>");
                        i = url_start + paren_close + 1;
                        continue;
                    }
                }
            }
        }

        // ── Plain text: escape special HTML chars ─────────────────────────────
        if chars[i] == '&' {
            let rest = &chars[i..];

            let entity_len = if matches!(rest.get(..5), Some(['&', 'a', 'm', 'p', ';'])) {
                Some(5)
            } else if matches!(rest.get(..6), Some(['&', 'q', 'u', 'o', 't', ';'])) {
                Some(6)
            } else if matches!(rest.get(..4), Some(['&', 'l', 't', ';'] | ['&', 'g', 't', ';'])) {
                Some(4)
            } else if rest.len() >= 4 && rest[0] == '&' && rest[1] == '#' {
                if let Some(end) = rest.iter().position(|&c| c == ';') {
                    let body = &rest[2..end];
                    let is_numeric_entity = if let Some(first) = body.first() {
                        if *first == 'x' || *first == 'X' {
                            body.len() > 1 && body[1..].iter().all(|c| c.is_ascii_hexdigit())
                        } else {
                            body.iter().all(|c| c.is_ascii_digit())
                        }
                    } else {
                        false
                    };

                    if is_numeric_entity {
                        Some(end + 1)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(len) = entity_len {
                out.extend(rest[..len].iter().copied());
                i += len;
            } else {
                out.push_str("&amp;");
                i += 1;
            }
            continue;
        }

        if chars[i] == '>' {
            out.push_str("&gt;");
            i += 1;
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

/// HTML-escape a string (for use inside code blocks and attribute values).
fn push_html_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// Return true if `s` (which starts with `<` and ends with `>`) is a
/// Telegram-supported HTML tag that should be passed through unchanged.
fn is_telegram_html_tag(s: &str) -> bool {
    // Strip the leading `<` (and optional `/` for closing tags)
    let inner = if let Some(rest) = s.strip_prefix("</") {
        rest
    } else if let Some(rest) = s.strip_prefix('<') {
        rest
    } else {
        return false;
    };
    // Extract the tag name (alphanumeric prefix)
    let tag_name: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    matches!(
        tag_name.to_lowercase().as_str(),
        "b" | "strong"
            | "i"
            | "em"
            | "u"
            | "ins"
            | "s"
            | "strike"
            | "del"
            | "code"
            | "pre"
            | "a"
            | "blockquote"
            | "tg-spoiler"
            | "tg-emoji"
    )
}

/// Split a message at paragraph/line boundaries so HTML tags are not cut.
///
/// Tries to split at `\n\n` (paragraph break), then at `\n` (line break),
/// and as a last resort at a safe UTF-8 character boundary near `max_len`.
#[derive(Clone)]
struct HtmlOpenTag {
    name: String,
    open_token: String,
}

#[derive(Clone)]
enum HtmlToken {
    Tag(String),
    Entity(String),
    Text(String),
}

fn tokenize_html_for_telegram(text: &str) -> Vec<HtmlToken> {
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < text.len() {
        let remaining = &text[i..];

        if remaining.starts_with('<') {
            if let Some(end) = remaining.find('>') {
                let token = &remaining[..=end];
                tokens.push(HtmlToken::Tag(token.to_string()));
                i += token.len();
                continue;
            }
        }

        if remaining.starts_with('&') {
            if let Some(end) = remaining.find(';') {
                let candidate = &remaining[..=end];
                if !candidate.contains('<') && !candidate.contains('>') && !candidate.contains('\n')
                {
                    tokens.push(HtmlToken::Entity(candidate.to_string()));
                    i += candidate.len();
                    continue;
                }
            }
        }

        let ch = remaining.chars().next().unwrap();
        tokens.push(HtmlToken::Text(ch.to_string()));
        i += ch.len_utf8();
    }

    tokens
}

fn parse_html_open_tag(tag: &str) -> Option<HtmlOpenTag> {
    if !tag.starts_with('<') || !tag.ends_with('>') {
        return None;
    }

    let inner = tag[1..tag.len() - 1].trim();
    if inner.is_empty()
        || inner.starts_with('/')
        || inner.starts_with('!')
        || inner.starts_with('?')
        || inner.ends_with('/')
    {
        return None;
    }

    let name: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();

    if name.is_empty() {
        return None;
    }

    Some(HtmlOpenTag {
        name,
        open_token: tag.to_string(),
    })
}

fn parse_html_close_tag_name(tag: &str) -> Option<String> {
    if !tag.starts_with("</") || !tag.ends_with('>') {
        return None;
    }

    let inner = tag[2..tag.len() - 1].trim();
    let name: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();

    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn apply_html_token_to_stack(stack: &mut Vec<HtmlOpenTag>, token: &HtmlToken) {
    if let HtmlToken::Tag(tag) = token {
        if let Some(close_name) = parse_html_close_tag_name(tag) {
            if let Some(pos) = stack.iter().rposition(|open| open.name == close_name) {
                stack.truncate(pos);
            }
        } else if let Some(open_tag) = parse_html_open_tag(tag) {
            stack.push(open_tag);
        }
    }
}

fn reopen_tags_html(stack: &[HtmlOpenTag]) -> String {
    stack.iter().map(|tag| tag.open_token.as_str()).collect()
}

fn close_tags_html(stack: &[HtmlOpenTag]) -> String {
    stack
        .iter()
        .rev()
        .map(|tag| format!("</{}>", tag.name))
        .collect()
}

fn split_at_boundary(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let tokens = tokenize_html_for_telegram(text);
    let mut chunks = Vec::new();
    let mut i = 0;
    let mut carry_stack: Vec<HtmlOpenTag> = Vec::new();

    while i < tokens.len() {
        let mut current = reopen_tags_html(&carry_stack);
        let mut current_stack = carry_stack.clone();
        let mut last_break: Option<(usize, String, Vec<HtmlOpenTag>)> = None;

        while i < tokens.len() {
            let token_str = match &tokens[i] {
                HtmlToken::Tag(s) | HtmlToken::Entity(s) | HtmlToken::Text(s) => s.as_str(),
            };

            let mut next_stack = current_stack.clone();
            apply_html_token_to_stack(&mut next_stack, &tokens[i]);
            let projected_len = current.len() + token_str.len() + close_tags_html(&next_stack).len();

            if projected_len <= max_len || current == reopen_tags_html(&carry_stack) {
                current.push_str(token_str);
                current_stack = next_stack;
                i += 1;

                if current.ends_with("\n\n") || current.ends_with('\n') {
                    last_break = Some((i, current.clone(), current_stack.clone()));
                }
            } else {
                break;
            }
        }

        if i < tokens.len() {
            if let Some((break_i, break_current, break_stack)) = last_break.clone() {
                let chunk = format!("{}{}", break_current, close_tags_html(&break_stack));
                chunks.push(chunk);
                i = break_i;
                carry_stack = break_stack;
                continue;
            }

            if current == reopen_tags_html(&carry_stack) {
                let token_str = match &tokens[i] {
                    HtmlToken::Tag(s) | HtmlToken::Entity(s) | HtmlToken::Text(s) => s.as_str(),
                };

                if let HtmlToken::Text(_) = &tokens[i] {
                    let available = max_len.saturating_sub(current.len() + close_tags_html(&current_stack).len());
                    let split_end = (0..=available.min(token_str.len()))
                        .rev()
                        .find(|&idx| token_str.is_char_boundary(idx))
                        .unwrap_or(0);

                    if split_end > 0 {
                        current.push_str(&token_str[..split_end]);
                        let chunk = format!("{}{}", current, close_tags_html(&current_stack));
                        chunks.push(chunk);

                        let remainder = &token_str[split_end..];
                        if !remainder.is_empty() {
                            let mut remaining_tokens = Vec::with_capacity(tokens.len() - i);
                            remaining_tokens.push(HtmlToken::Text(remainder.to_string()));
                            remaining_tokens.extend_from_slice(&tokens[i + 1..]);
                            let mut rebuilt = Vec::with_capacity(i + remaining_tokens.len());
                            rebuilt.extend_from_slice(&tokens[..i]);
                            rebuilt.extend(remaining_tokens);
                            return {
                                let mut recursive_chunks = chunks;
                                recursive_chunks.extend(split_at_boundary(
                                    &rebuilt
                                        .into_iter()
                                        .map(|token| match token {
                                            HtmlToken::Tag(s)
                                            | HtmlToken::Entity(s)
                                            | HtmlToken::Text(s) => s,
                                        })
                                        .collect::<String>(),
                                    max_len,
                                ));
                                recursive_chunks
                            };
                        }
                    }
                }
            }

            let chunk = format!("{}{}", current, close_tags_html(&current_stack));
            chunks.push(chunk);
            carry_stack = current_stack;
        } else {
            let chunk = format!("{}{}", current, close_tags_html(&current_stack));
            if !chunk.is_empty() {
                chunks.push(chunk);
            }
        }
    }
    chunks
}

#[derive(Debug, Clone)]
struct SubagentStatusRow {
    id: String,
    status: String,
    depth: i32,
    model: Option<String>,
}

async fn send_response_text(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    text: &str,
) -> Result<(), teloxide::RequestError> {
    if text.is_empty() {
        return Ok(());
    }

    let html = convert_to_telegram_html(text);

    if html.len() <= 4000 {
        return send_message_formatted(bot, chat_id, html).await;
    }

    // Long message: split at natural boundaries and send each chunk with HTML.
    for chunk in split_at_boundary(&html, 4000) {
        if !chunk.is_empty() {
            send_message_formatted(bot, chat_id, chunk).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_escape() {
        let mut out = String::new();
        push_html_escaped(&mut out, "a & b < c > d \"e\"");
        assert_eq!(out, "a &amp; b &lt; c &gt; d &quot;e&quot;");
    }

    #[test]
    fn test_convert_fenced_code_block() {
        let input = "```rust\nfn main() {}\n```";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "<pre>fn main() {}</pre>");
    }

    #[test]
    fn test_convert_inline_code() {
        let input = "Use `cargo build` to compile.";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "Use <code>cargo build</code> to compile.");
    }

    #[test]
    fn test_convert_bold() {
        let input = "This is **bold** text.";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "This is <b>bold</b> text.");
    }

    #[test]
    fn test_convert_italic() {
        let input = "This is *italic* text.";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "This is <i>italic</i> text.");
    }

    #[test]
    fn test_convert_strikethrough() {
        let input = "This is ~~strikethrough~~ text.";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "This is <s>strikethrough</s> text.");
    }

    #[test]
    fn test_convert_link() {
        let input = "See [docs](https://example.com) for details.";
        let html = convert_to_telegram_html(input);
        assert_eq!(
            html,
            "See <a href=\"https://example.com\">docs</a> for details."
        );
    }

    #[test]
    fn test_passthrough_existing_html_tags() {
        let input = "Hello <b>world</b> and <code>code</code>.";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "Hello <b>world</b> and <code>code</code>.");
    }

    #[test]
    fn test_escape_plain_text_special_chars() {
        let input = "a & b, x < y, p > q";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "a &amp; b, x &lt; y, p &gt; q");
    }

    #[test]
    fn test_escape_unknown_html_tag() {
        // Unknown tags should be escaped, not passed through.
        let input = "Use <script>alert(1)</script>";
        let html = convert_to_telegram_html(input);
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_is_telegram_html_tag() {
        assert!(is_telegram_html_tag("<b>"));
        assert!(is_telegram_html_tag("</b>"));
        assert!(is_telegram_html_tag("<code>"));
        assert!(is_telegram_html_tag("<a href=\"url\">"));
        assert!(!is_telegram_html_tag("<script>"));
        assert!(!is_telegram_html_tag("<div>"));
    }

    #[test]
    fn test_split_at_boundary_short_message() {
        let text = "short message";
        let chunks = split_at_boundary(text, 100);
        assert_eq!(chunks, vec!["short message"]);
    }

    #[test]
    fn test_split_at_boundary_long_message() {
        let line = "a".repeat(100);
        let text = format!("{}\n{}\n{}", line, line, line);
        // Total length = 302; with max_len=150, each 100-char line is split individually.
        let chunks = split_at_boundary(&text, 150);
        // Each chunk must fit within the limit
        for chunk in &chunks {
            assert!(chunk.len() <= 150, "chunk too long: {}", chunk.len());
        }
        // All chunks combined should contain all the content
        let combined = chunks.join("\n");
        assert!(combined.contains(&line));
    }

    #[test]
    fn test_split_at_boundary_hard_cut() {
        // No newlines: must fall back to hard-cut at the safe UTF-8 boundary.
        let text = "a".repeat(500);
        let chunks = split_at_boundary(&text, 100);
        assert_eq!(chunks.len(), 5);
        for chunk in &chunks {
            assert_eq!(chunk.len(), 100);
        }
    }

    #[test]
    fn test_split_at_boundary_unicode_safe() {
        // Each '💡' emoji is 4 bytes. 10 emojis = 40 bytes.
        // With max_len=15 (not on a char boundary after the 3rd emoji at byte 12),
        // the function must not panic and must not produce a broken string.
        let text = "💡".repeat(10);
        let chunks = split_at_boundary(&text, 15);
        for chunk in &chunks {
            // Every chunk must be valid UTF-8 (String from a &str is always valid,
            // but the slice boundary must be correct).
            assert!(!chunk.is_empty());
            assert!(chunk.chars().all(|c| c == '💡'));
        }
    }
}
