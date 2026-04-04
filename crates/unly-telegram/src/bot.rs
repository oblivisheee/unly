use base64::Engine;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use teloxide::{
    net::Download,
    prelude::*,
    types::{
        CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, InputFile, Message, ParseMode,
    },
    utils::command::BotCommands,
};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use unly_agent::{
    AgentContext, AgentResponse, AgentRuntime, MediaKind, StreamEvent, SubagentManager,
    SubagentRequest, SubagentSpawnConfig,
};
use unly_audit::AuditLogger;
use unly_config::{workspace, AppConfig, DbType};
use unly_core::ids::ChatId;
use unly_core::model::{ChatMessageContent, ContentPart, ImageUrl};
use unly_db::Database;
use unly_providers::ProviderRegistry;
use unly_tools::builtin::{
    orchestration::build_notify_message, register_cron_executor, register_subagent_executor,
};

use crate::{
    commands::Command,
    permissions::{build_permissions, is_allowed},
    session::{PendingSubagentSpawn, SessionStore},
};

/// The main Telegram bot handler.
pub struct TelegramBot {
    config: Arc<AppConfig>,
    sessions: SessionStore,
    runtime: Arc<AgentRuntime>,
    provider_registry: Arc<ProviderRegistry>,
    db: Database,
    audit: Arc<AuditLogger>,
    subagents: Arc<SubagentManager>,
}

impl TelegramBot {
    fn default_auto_approve(&self) -> bool {
        let policy = self.runtime.tool_policy();
        !policy.require_approval_for_privileged && !policy.require_approval_for_dangerous
    }

    fn effective_auto_approve(&self) -> bool {
        self.sessions
            .global_auto_approve()
            .unwrap_or_else(|| self.default_auto_approve())
    }

    pub fn new(
        config: Arc<AppConfig>,
        sessions: SessionStore,
        runtime: Arc<AgentRuntime>,
        provider_registry: Arc<ProviderRegistry>,
        db: Database,
        audit: Arc<AuditLogger>,
    ) -> Self {
        let subagent_cfg = SubagentSpawnConfig {
            max_depth: config.agent.max_subagent_depth,
            max_concurrent: config.agent.max_concurrent_subagents,
            max_children_per_parent: config.agent.max_child_subagents_per_parent,
            token_budget: config.agent.subagent_token_budget,
        };
        let subagents = Arc::new(SubagentManager::new(subagent_cfg, db.clone()));
        let runtime_for_subagent_tool = runtime.clone();
        let runtime_for_cron_tool = runtime.clone();
        let subagents_for_tool = subagents.clone();
        let db_for_tool = db.clone();
        let db_for_cron = db.clone();
        let token_for_tool = config.telegram.bot_token.clone();
        let token_for_cron = config.telegram.bot_token.clone();
        let default_provider = config.providers.default_provider.clone();
        let default_model = config.providers.default_model.clone();
        let subagent_default_provider = default_provider.clone();
        let subagent_default_model = default_model.clone();
        let cron_default_provider = default_provider.clone();
        let cron_default_model = default_model.clone();
        let subagent_token_budget = config.agent.subagent_token_budget;

        register_subagent_executor(Arc::new(
            move |goal, chat_id, provider, model, permissions, parent_agent_id| {
                let subagents = subagents_for_tool.clone();
                let runtime = runtime_for_subagent_tool.clone();
                let db = db_for_tool.clone();
                let token = token_for_tool.clone();
                let provider_fallback = subagent_default_provider.clone();
                let model_fallback = subagent_default_model.clone();
                Box::pin(async move {
                    let is_child_subagent = parent_agent_id.is_some();
                    let parent_agent_id = parent_agent_id.unwrap_or_default();
                    let request = SubagentRequest {
                        goal: goal.clone(),
                        parent_agent_id,
                        depth: 0,
                        permissions,
                        provider: Some(provider.unwrap_or(provider_fallback)),
                        model: Some(model.unwrap_or(model_fallback)),
                        token_budget: subagent_token_budget,
                    };
                    let handle = subagents
                        .spawn_background(request, runtime, chat_id)
                        .await
                        .map_err(|e| e.to_string())?;

                    if let Some(tg_chat_id) = resolve_telegram_chat_id(&db, chat_id).await {
                        let bot = Bot::new(token);
                        let _ = bot
                            .send_message(
                                teloxide::types::ChatId(tg_chat_id),
                                format!("Subagent spawned with task: {}", shorten_goal(&goal)),
                            )
                            .await;
                        if !is_child_subagent {
                            let id = handle.id.to_string();
                            let subagents = subagents.clone();
                            tokio::spawn(async move {
                                wait_and_notify_subagent_result(
                                    &bot,
                                    &subagents,
                                    teloxide::types::ChatId(tg_chat_id),
                                    id,
                                )
                                .await;
                            });
                        }
                    }
                    Ok(handle.id.to_string())
                })
            },
        ));

        register_cron_executor(Arc::new(
            move |task, chat_id, telegram_chat_id, notify_mode, trigger| {
                let runtime = runtime_for_cron_tool.clone();
                let db = db_for_cron.clone();
                let token = token_for_cron.clone();
                let provider = cron_default_provider.clone();
                let model = cron_default_model.clone();
                Box::pin(async move {
                    let mut ctx = AgentContext::new(
                        chat_id,
                        None,
                        unly_core::permissions::PermissionSet::admin(),
                        provider,
                        model,
                        runtime.config().system_prompt.clone(),
                    );
                    let result_text = match runtime.process(&mut ctx, task.clone()).await {
                        Ok(AgentResponse::Text(text)) => text,
                        Ok(AgentResponse::ApprovalRequired { .. }) => {
                            match runtime.process_approved(&mut ctx).await {
                                Ok(AgentResponse::Text(text)) => text,
                                Ok(AgentResponse::ApprovalRequired { pending }) => {
                                    format!("approval required for {} tool calls", pending.len())
                                }
                                Err(e) => return Err(e.to_string()),
                            }
                        }
                        Err(e) => return Err(e.to_string()),
                    };
                    if notify_mode != "silent" {
                        let resolved_tg_chat_id = match telegram_chat_id {
                            Some(id) => Some(id),
                            None => resolve_telegram_chat_id(&db, chat_id).await,
                        };
                        if let Some(tg_chat_id) = resolved_tg_chat_id {
                            let bot = Bot::new(token);
                            let notify = build_notify_message(&task, &trigger, &result_text);
                            let _ = bot
                                .send_message(teloxide::types::ChatId(tg_chat_id), notify)
                                .await;
                        }
                    }
                    Ok(result_text)
                })
            },
        ));

        Self {
            config,
            sessions,
            runtime,
            provider_registry,
            subagents,
            db,
            audit,
        }
    }

    /// Start the Telegram bot polling loop.
    pub async fn start(self: Arc<Self>) {
        let token = self.config.telegram.bot_token.clone();
        let bot = Bot::new(token);

        if let Err(e) = bot.set_my_commands(Command::bot_commands()).await {
            warn!(error = %e, "failed to set native Telegram command list");
        }

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
            bot.send_message(msg.chat.id, "You are not authorized to use this bot.")
                .await?;
            return Ok(());
        }

        match cmd {
            Command::Start => {
                info!(user = tg_user_id, chat_id = tg_chat_id, "session started");
                self.sessions.remove(tg_chat_id);
                self.sessions.mark_skip_history_restore(tg_chat_id);
                if workspace::is_boot_mode() {
                    let boot_start = self.generate_boot_start_message(tg_user_id).await;
                    send_response_text(&bot, msg.chat.id, &boot_start).await?;
                    let _ = workspace::mark_boot_prompted();
                } else {
                    let start_text = self.generate_start_message(tg_user_id).await;
                    send_response_text(&bot, msg.chat.id, &start_text).await?;
                }
            }
            Command::New => {
                info!(user = tg_user_id, chat_id = tg_chat_id, "session restarted");
                self.sessions.remove(tg_chat_id);
                self.sessions.mark_skip_history_restore(tg_chat_id);
                if workspace::is_boot_mode() {
                    let boot_start = self.generate_boot_start_message(tg_user_id).await;
                    send_response_text(&bot, msg.chat.id, &boot_start).await?;
                    let _ = workspace::mark_boot_prompted();
                } else {
                    let start_text = self.generate_start_message(tg_user_id).await;
                    send_response_text(&bot, msg.chat.id, &start_text).await?;
                }
            }
            Command::Reset => {
                info!(user = tg_user_id, chat_id = tg_chat_id, "session reset");
                self.sessions.remove(tg_chat_id);
                self.sessions.mark_skip_history_restore(tg_chat_id);
                bot.send_message(msg.chat.id, "Session reset. Send your next request.")
                    .await?;
            }

            Command::Help => {
                let text = format!(
                "{}\n\nTip: send text, documents, or photos — I can process attachments directly.",
                Command::descriptions()
            );
                bot.send_message(msg.chat.id, text).await?;
            }

            Command::Status => {
                let reports = self.provider_registry.health_all().await;
                let mut lines = vec!["System status".to_string()];
                for r in &reports {
                    let health = match r.status {
                        unly_core::types::HealthStatus::Healthy => "healthy",
                        unly_core::types::HealthStatus::Degraded => "degraded",
                        unly_core::types::HealthStatus::Unhealthy => "unhealthy",
                        unly_core::types::HealthStatus::Unknown => "unknown",
                    };
                    lines.push(format!(
                        "{}: {} ({})",
                        r.name,
                        r.message.as_deref().unwrap_or("ok"),
                        health
                    ));
                }
                let sessions = self.sessions.len();
                lines.push(format!("Active sessions: {}", sessions));

                // Show current approval mode.
                let auto_approve = self.effective_auto_approve();
                let approval_mode = if auto_approve {
                    "auto (prompts disabled)"
                } else {
                    "manual (tool approval prompts)"
                };
                lines.push(format!("Approval mode: {}", approval_mode));

                // Show active job count from DB.
                let job_repo = unly_db::repo::job::JobRepo::new(self.db.conn());
                match job_repo.list_enabled().await {
                    Ok(jobs) => {
                        lines.push(format!("Active cron jobs: {}", jobs.len()));
                    }
                    Err(_) => {
                        lines.push("Active cron jobs: unavailable".to_string());
                    }
                }

                // Show current provider/model for this session.
                if let Some(ctx) = self.sessions.get(tg_chat_id) {
                    lines.push(format!("Provider/model: {}/{}", ctx.provider, ctx.model));
                } else {
                    let default_provider = self
                        .provider_registry
                        .default_provider()
                        .map(|p| p.name().to_string())
                        .unwrap_or_else(|_| "copilot".to_string());
                    let default_model = self.provider_registry.default_model();
                    lines.push(format!(
                        "Provider/model (default): {}/{}",
                        default_provider, default_model
                    ));
                }

                bot.send_message(msg.chat.id, lines.join("\n")).await?;
            }

            Command::Model(model_id) => {
                info!(
                    user = tg_user_id,
                    chat_id = tg_chat_id,
                    model = %model_id,
                    "user changed active model"
                );
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
                    info!(
                        user = tg_user_id,
                        chat_id = tg_chat_id,
                        provider = %provider_name,
                        "user changed active provider"
                    );
                    if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                        ctx.provider = provider_name.clone();
                        self.sessions.set(tg_chat_id, ctx);
                    }
                    bot.send_message(msg.chat.id, format!("Provider set to {}", provider_name))
                        .await?;
                } else {
                    let available = self.provider_registry.provider_names().join(", ");
                    warn!(
                        user = tg_user_id,
                        chat_id = tg_chat_id,
                        provider = %provider_name,
                        available = %available,
                        "user requested unknown provider"
                    );
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

            Command::Subagents => {
                self.send_subagents_menu(&bot, msg.chat.id, SubagentMenuView::Active)
                    .await?;
            }

            Command::Approve => {
                if self.sessions.has_pending_subagent(tg_chat_id) {
                    self.approve_pending_subagent(&bot, msg.chat.id, tg_chat_id)
                        .await?;
                    return Ok(());
                }
                if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                    if ctx.pending_approvals.is_empty() {
                        bot.send_message(msg.chat.id, "No pending approvals.")
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
                    if let Err(e) = drain_pending_media(&bot, msg.chat.id, &mut ctx).await {
                        error!(chat_id = tg_chat_id, error = %e, "failed to send approved media");
                    }
                    self.sessions.set(tg_chat_id, ctx);

                    match response {
                        Ok(AgentResponse::Text(text)) => {
                            send_response_text(&bot, msg.chat.id, &text).await?;
                        }
                        Ok(AgentResponse::ApprovalRequired { pending }) => {
                            bot.send_message(msg.chat.id, format_approval_prompt(&pending))
                                .parse_mode(ParseMode::Html)
                                .await?;
                        }
                        Err(e) => {
                            bot.send_message(msg.chat.id, format!("Error after approval: {}", e))
                                .await?;
                        }
                    }
                } else {
                    bot.send_message(msg.chat.id, "No active session.").await?;
                }
            }

            Command::Deny => {
                if self.sessions.has_pending_subagent(tg_chat_id) {
                    info!(
                        user = tg_user_id,
                        chat_id = tg_chat_id,
                        "user denied pending subagent spawn"
                    );
                    let _ = self.sessions.take_pending_subagent(tg_chat_id);
                    bot.send_message(msg.chat.id, "Subagent creation denied.")
                        .await?;
                    return Ok(());
                }
                if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                    let pending = std::mem::take(&mut ctx.pending_approvals);
                    self.sessions.set(tg_chat_id, ctx);
                    if pending.is_empty() {
                        bot.send_message(msg.chat.id, "No pending approvals.")
                            .await?;
                    } else {
                        let names: Vec<&str> =
                            pending.iter().map(|p| p.tool_name.as_str()).collect();
                        info!(
                            user = tg_user_id,
                            chat_id = tg_chat_id,
                            tools = ?names,
                            "user denied tool executions"
                        );
                        self.audit.denied(
                            "tool_approval",
                            tg_user_id.to_string(),
                            format!("denied: {}", names.join(", ")),
                            "user denied",
                        );
                        bot.send_message(
                            msg.chat.id,
                            format!("Denied tool executions: {}", names.join(", ")),
                        )
                        .await?;
                    }
                } else {
                    bot.send_message(msg.chat.id, "No active session.").await?;
                }
            }
            Command::Approval(mode) => match mode.trim().to_lowercase().as_str() {
                "auto" => {
                    info!(
                        user = tg_user_id,
                        chat_id = tg_chat_id,
                        mode = "auto",
                        "approval mode changed"
                    );
                    self.audit
                        .success("approval_mode", tg_user_id.to_string(), "set mode=auto");
                    self.sessions.set_global_auto_approve(true);
                    if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                        ctx.tool_approval_override = Some(true);
                        self.sessions.set(tg_chat_id, ctx);
                    }
                    let policy = self.runtime.tool_policy();
                    let affected: Vec<&str> = [
                        if policy.require_approval_for_privileged {
                            Some("privileged")
                        } else {
                            None
                        },
                        if policy.require_approval_for_dangerous {
                            Some("dangerous")
                        } else {
                            None
                        },
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    let affected_str = if affected.is_empty() {
                        "Global policy does not require approvals.".to_string()
                    } else {
                        format!(
                            "Global policy requires approval for {} tools.",
                            affected.join(" and ")
                        )
                    };
                    bot.send_message(
                        msg.chat.id,
                        format!(
                            "Approval mode set to auto.\n\
                             {affected_str}\n\
                             Tool calls will run without approval prompts."
                        ),
                    )
                    .await?;
                }
                "manual" => {
                    info!(
                        user = tg_user_id,
                        chat_id = tg_chat_id,
                        mode = "manual",
                        "approval mode changed"
                    );
                    self.audit
                        .success("approval_mode", tg_user_id.to_string(), "set mode=manual");
                    self.sessions.set_global_auto_approve(false);
                    if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                        ctx.tool_approval_override = Some(false);
                        self.sessions.set(tg_chat_id, ctx);
                    }
                    let policy = self.runtime.tool_policy();
                    let affected: Vec<&str> = [
                        if policy.require_approval_for_privileged {
                            Some("privileged")
                        } else {
                            None
                        },
                        if policy.require_approval_for_dangerous {
                            Some("dangerous")
                        } else {
                            None
                        },
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    let affected_str = if affected.is_empty() {
                        "Global policy does not require additional tool approvals, but manual mode is active".to_string()
                    } else {
                        format!(
                            "Global policy requires approval for {} tools",
                            affected.join(" and ")
                        )
                    };
                    bot.send_message(
                        msg.chat.id,
                        format!(
                            "Approval mode set to manual.\n\
                             {affected_str}.\n\
                             Tool calls will ask for confirmation before execution.\n\
                             Use /approval auto to switch back."
                        ),
                    )
                    .await?;
                }
                "" => {
                    // Show current mode when called without argument.
                    let auto = self.effective_auto_approve();
                    let mode_str = if auto { "AUTO" } else { "MANUAL" };
                    bot.send_message(
                        msg.chat.id,
                        format!(
                            "Current approval mode: {mode_str}\n\
                             Use /approval auto or /approval manual to change."
                        ),
                    )
                    .await?;
                }
                other => {
                    warn!(
                        user = tg_user_id,
                        chat_id = tg_chat_id,
                        invalid_mode = other,
                        "invalid approval mode requested"
                    );
                    bot.send_message(
                        msg.chat.id,
                        "Invalid approval mode. Use: /approval manual | /approval auto | /approval (show current)",
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
            warn!(
                user = tg_user_id,
                chat_id = tg_chat_id,
                "unauthorized message blocked"
            );
            bot.send_message(msg.chat.id, "You are not authorized to use this bot.")
                .await?;
            return Ok(());
        }

        let user_input = match self.build_user_input_from_message(&bot, &msg).await {
            Ok(Some(input)) => input,
            Ok(None) => return Ok(()),
            Err(e) => {
                error!(chat_id = tg_chat_id, error = %e, "failed to parse telegram attachments");
                bot.send_message(
                    msg.chat.id,
                    "Could not process the attached file/photo. Please try again.",
                )
                .await?;
                return Ok(());
            }
        };
        let text = user_input_for_storage(&user_input);

        if self.sessions.has_pending_subagent(tg_chat_id) && is_affirmative_approval(&text) {
            self.approve_pending_subagent(&bot, msg.chat.id, tg_chat_id)
                .await?;
            return Ok(());
        }
        if let Some(mut pending_ctx) = self.sessions.get(tg_chat_id)
            && !pending_ctx.pending_approvals.is_empty()
        {
            if self.effective_auto_approve() {
                let mut response = self.runtime.process_approved(&mut pending_ctx).await;
                for _ in 0..8 {
                    match response {
                        Ok(AgentResponse::ApprovalRequired { .. }) => {
                            response = self.runtime.process_approved(&mut pending_ctx).await;
                        }
                        _ => break,
                    }
                }
                if let Err(e) = drain_pending_media(&bot, msg.chat.id, &mut pending_ctx).await {
                    error!(chat_id = tg_chat_id, error = %e, "failed to send approved media");
                }
                self.sessions.set(tg_chat_id, pending_ctx);
                match response {
                    Ok(AgentResponse::Text(answer)) => {
                        send_response_text(&bot, msg.chat.id, &answer).await?;
                    }
                    Ok(AgentResponse::ApprovalRequired { pending }) => {
                        let keyboard = InlineKeyboardMarkup::new(vec![vec![
                            InlineKeyboardButton::callback("Approve", "approve"),
                            InlineKeyboardButton::callback("Deny", "deny"),
                        ]]);
                        bot.send_message(msg.chat.id, format_approval_prompt(&pending))
                            .parse_mode(ParseMode::Html)
                            .reply_markup(keyboard)
                            .await?;
                    }
                    Err(e) => {
                        bot.send_message(msg.chat.id, format!("Error after approval: {}", e))
                            .await?;
                    }
                }
                return Ok(());
            }
            if is_affirmative_approval(&text) {
                let response = self.runtime.process_approved(&mut pending_ctx).await;
                if let Err(e) = drain_pending_media(&bot, msg.chat.id, &mut pending_ctx).await {
                    error!(chat_id = tg_chat_id, error = %e, "failed to send approved media");
                }
                self.sessions.set(tg_chat_id, pending_ctx);
                match response {
                    Ok(AgentResponse::Text(answer)) => {
                        send_response_text(&bot, msg.chat.id, &answer).await?;
                    }
                    Ok(AgentResponse::ApprovalRequired { pending }) => {
                        let keyboard = InlineKeyboardMarkup::new(vec![vec![
                            InlineKeyboardButton::callback("Approve", "approve"),
                            InlineKeyboardButton::callback("Deny", "deny"),
                        ]]);
                        bot.send_message(msg.chat.id, format_approval_prompt(&pending))
                            .reply_markup(keyboard)
                            .await?;
                    }
                    Err(e) => {
                        bot.send_message(msg.chat.id, format!("Error after approval: {}", e))
                            .await?;
                    }
                }
                return Ok(());
            }
            if is_negative_approval(&text) {
                let denied = std::mem::take(&mut pending_ctx.pending_approvals);
                self.sessions.set(tg_chat_id, pending_ctx);
                let details = format_pending_approvals(&denied);
                bot.send_message(msg.chat.id, format!("Denied tool executions:\n{}", details))
                    .await?;
                return Ok(());
            }
        }

        if workspace::is_boot_mode() {
            if self.should_finalize_boot(tg_user_id, &text).await {
                let summary = self
                    .build_boot_profile_update(tg_user_id)
                    .await
                    .unwrap_or_else(build_boot_summary);
                match workspace::finalize_boot(&summary) {
                    Ok(()) => {
                        let done_text = self.generate_boot_done_message(tg_user_id).await;
                        bot.send_message(msg.chat.id, done_text).await?;
                        return Ok(());
                    }
                    Err(e) => {
                        error!(chat_id = tg_chat_id, error = %e, "failed to finalize boot");
                        bot.send_message(
                            msg.chat.id,
                            "Failed to finalize BOOT profile. Please try again.",
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

        // Resolve persistent chat row first so runtime ChatId is bound to DB chat id.
        let chat_repo = unly_db::repo::chat::ChatRepo::new(self.db.conn());
        let chat_row = chat_repo
            .get_or_create_chat(tg_chat_id, msg.chat.title().or(msg.chat.username()))
            .await;

        // Get or create session context.
        // If this is a fresh context (no in-memory session), try to restore
        // conversation history from the database so the agent remembers
        // previous exchanges after a restart.
        let mut ctx = match self.sessions.get(tg_chat_id) {
            Some(existing) => existing,
            None => {
                let permissions =
                    build_permissions(tg_user_id, &self.config.telegram.admin_user_ids);
                let chat_id = chat_row
                    .as_ref()
                    .ok()
                    .and_then(|row| ChatId::from_str(&row.id).ok())
                    .unwrap_or_else(ChatId::new);
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
            }
        };
        ctx.tool_approval_override = Some(self.effective_auto_approve());

        // Keep in-memory context bound to persisted chat id even for old sessions.
        if let Ok(row) = &chat_row
            && let Ok(bound_chat_id) = ChatId::from_str(&row.id)
            && ctx.chat_id != bound_chat_id
        {
            ctx.chat_id = bound_chat_id;
        }

        // Restore message history from DB when the in-memory session is empty.
        // Explicit conversation reset commands skip history restore once.
        let skip_history_restore = self.sessions.take_skip_history_restore(tg_chat_id);
        if ctx.messages.is_empty() && !skip_history_restore
            && let Ok(chat_row_hist) = &chat_row
        {
            // Load the last 40 messages (20 turns) to bound context size.
            if let Ok(history) = chat_repo.list_messages(&chat_row_hist.id, 40).await {
                for row in history {
                    let text_content = serde_json::from_str::<serde_json::Value>(&row.content)
                        .ok()
                        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string))
                        .unwrap_or(row.content);
                    ctx.push_message(unly_core::model::ChatMessage {
                        role: row.role,
                        content: unly_core::model::ChatMessageContent::Text(text_content),
                        tool_call_id: None,
                        tool_calls: None,
                        name: None,
                    });
                }
            }
        }

        let trimmed = text.trim();
        let subagent_goal = parse_spawn_subagent_request(trimmed);
        if let Some(goal) = subagent_goal {
            // Persist session immediately so /approve can always resolve chat context.
            self.sessions.set(tg_chat_id, ctx.clone());
            self.sessions.set_pending_subagent(
                tg_chat_id,
                PendingSubagentSpawn {
                    goal: goal.clone(),
                    parent_agent_id: ctx.agent_id,
                    depth: ctx.subagent_depth,
                    provider: ctx.provider.clone(),
                    model: ctx.model.clone(),
                },
            );
            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("Approve", "approve"),
                InlineKeyboardButton::callback("Deny", "deny"),
            ]]);
            bot.send_message(
                msg.chat.id,
                format!(
                    "Confirm subagent creation for task: {}\nThis will grant the subagent full command/tool permissions.",
                    shorten_goal(&goal)
                ),
            )
            .reply_markup(keyboard)
            .await?;
            return Ok(());
        }

        // Send a "typing..." indicator.
        bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
            .await?;
        let typing_bot = bot.clone();
        let typing_chat = msg.chat.id;
        let (typing_stop_tx, mut typing_stop_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(4));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let _ = typing_bot
                            .send_chat_action(typing_chat, teloxide::types::ChatAction::Typing)
                            .await;
                    }
                    changed = typing_stop_rx.changed() => {
                        if changed.is_err() || *typing_stop_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

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

        let input_clone = user_input.clone();
        tokio::spawn(async move {
            if let Err(e) = runtime
                .process_stream_input(&mut ctx_clone, input_clone, tx)
                .await
            {
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
                    let _ = typing_stop_tx.send(true);
                    return Ok(());
                }
                StreamEvent::SendMedia {
                    kind,
                    path,
                    caption,
                } => {
                    if let Err(e) =
                        send_media(&bot, msg.chat.id, &kind, &path, caption.as_deref()).await
                    {
                        error!(
                            chat_id = tg_chat_id,
                            error = %e,
                            media_kind = ?kind,
                            media_path = %path,
                            "failed to send streamed media"
                        );
                    }
                }
                StreamEvent::ApprovalRequired {
                    pending,
                    ctx: event_ctx,
                } => {
                    let _ = typing_stop_tx.send(true);
                    if self.effective_auto_approve() {
                        // Use the context snapshot from the event rather than
                        // fetching from sessions to avoid a race where the
                        // background task has not yet called sessions.set().
                        let mut ctx = *event_ctx;
                        let mut response = self.runtime.process_approved(&mut ctx).await;
                        for _ in 0..8 {
                            match response {
                                Ok(AgentResponse::ApprovalRequired { .. }) => {
                                    response = self.runtime.process_approved(&mut ctx).await;
                                }
                                _ => break,
                            }
                        }
                        if let Err(e) = drain_pending_media(&bot, msg.chat.id, &mut ctx).await {
                            error!(
                                chat_id = tg_chat_id,
                                error = %e,
                                "failed to send auto-approved media"
                            );
                        }
                        self.sessions.set(tg_chat_id, ctx);
                        match response {
                            Ok(AgentResponse::Text(text)) => {
                                if let Err(e) = send_response_text(&bot, msg.chat.id, &text).await {
                                    error!(
                                        chat_id = tg_chat_id,
                                        error = %e,
                                        "failed to send auto-approved response"
                                    );
                                }
                            }
                            Ok(AgentResponse::ApprovalRequired { pending: p }) => {
                                let keyboard = InlineKeyboardMarkup::new(vec![vec![
                                    InlineKeyboardButton::callback("Approve", "approve"),
                                    InlineKeyboardButton::callback("Deny", "deny"),
                                ]]);
                                let _ = bot
                                    .send_message(msg.chat.id, format_approval_prompt(&p))
                                    .parse_mode(ParseMode::Html)
                                    .reply_markup(keyboard)
                                    .await;
                            }
                            Err(e) => {
                                let _ = bot
                                    .send_message(
                                        msg.chat.id,
                                        format!("Error after approval: {}", e),
                                    )
                                    .await;
                            }
                        }
                        return Ok(());
                    }
                    // Manual approval: save the context snapshot to sessions so
                    // handle_callback can resume from the correct state.
                    self.sessions.set(tg_chat_id, *event_ctx);
                    let keyboard = InlineKeyboardMarkup::new(vec![vec![
                        InlineKeyboardButton::callback("Approve", "approve"),
                        InlineKeyboardButton::callback("Deny", "deny"),
                    ]]);
                    if let Err(e) = bot
                        .send_message(msg.chat.id, format_approval_prompt(&pending))
                        .parse_mode(ParseMode::Html)
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
        let _ = typing_stop_tx.send(true);
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

    async fn load_subagents(&self) -> Vec<SubagentStatusRow> {
        let sql = "SELECT id, status, depth, goal, model, updated_at, parent_agent_id \
                   FROM subagents \
                   ORDER BY updated_at DESC \
                   LIMIT 4";
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
                    goal: r.try_get("", "goal").ok(),
                    model: r.try_get("", "model").ok(),
                    updated_at: r.try_get("", "updated_at").ok(),
                })
                .collect(),
            Err(e) => {
                error!(error = %e, "failed to query subagents");
                Vec::new()
            }
        }
    }

    async fn build_user_input_from_message(
        &self,
        bot: &Bot,
        msg: &Message,
    ) -> Result<Option<ChatMessageContent>, Box<dyn std::error::Error + Send + Sync>> {
        let text = msg
            .text()
            .or(msg.caption())
            .map(|t| t.to_string())
            .unwrap_or_default();
        let mut parts: Vec<ContentPart> = Vec::new();
        if !text.trim().is_empty() {
            parts.push(ContentPart::Text { text });
        }

        if let Some(doc) = msg.document() {
            let file = bot.get_file(doc.file.id.clone()).await?;
            let mut bytes = Vec::new();
            bot.download_file(&file.path, &mut bytes).await?;
            let mime = doc
                .mime_type
                .as_ref()
                .map(|m| m.essence_str().to_string())
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let name = doc
                .file_name
                .clone()
                .unwrap_or_else(|| "attachment".to_string());
            if mime.starts_with("image/") {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let uri = format!("data:{};base64,{}", mime, b64);
                parts.push(ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: uri,
                        detail: Some("high".to_string()),
                    },
                });
            } else if let Ok(text_content) = String::from_utf8(bytes.clone()) {
                let snippet: String = text_content.chars().take(24_000).collect();
                parts.push(ContentPart::Text {
                    text: format!("Attached file `{}` content:\n{}", name, snippet),
                });
            } else {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let snippet: String = b64.chars().take(32_000).collect();
                parts.push(ContentPart::Text {
                    text: format!(
                        "Attached binary file `{}` (mime: {}). Base64 preview:\n{}",
                        name, mime, snippet
                    ),
                });
            }
            parts.push(ContentPart::Text {
                text: format!("[Attached file: {}]", name),
            });
        }

        if let Some(photos) = msg.photo()
            && let Some(photo) = photos.last()
        {
            let file = bot.get_file(photo.file.id.clone()).await?;
            let mut bytes = Vec::new();
            bot.download_file(&file.path, &mut bytes).await?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let uri = format!("data:image/jpeg;base64,{}", b64);
            parts.push(ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: uri,
                    detail: Some("high".to_string()),
                },
            });
        }

        if parts.is_empty() {
            return Ok(None);
        }
        if parts.len() == 1
            && let ContentPart::Text { text } = &parts[0]
        {
            return Ok(Some(ChatMessageContent::Text(text.clone())));
        }
        Ok(Some(ChatMessageContent::Parts(parts)))
    }

    async fn handle_callback(
        &self,
        bot: Bot,
        q: CallbackQuery,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(data) = q.data.as_deref() else {
            return Ok(());
        };
        let Some(ref message) = q.message else {
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
        let mut delete_callback_message = true;
        match data {
            "approve" => {
                handled = true;
                if self.sessions.has_pending_subagent(tg_chat_id) {
                    self.approve_pending_subagent(&bot, message.chat().id, tg_chat_id)
                        .await?;
                } else if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
                    if ctx.pending_approvals.is_empty() {
                        bot.send_message(message.chat().id, "No pending approvals.")
                            .await?;
                    } else {
                        let response = self.runtime.process_approved(&mut ctx).await;
                        if let Err(e) = drain_pending_media(&bot, message.chat().id, &mut ctx).await
                        {
                            error!(chat_id = tg_chat_id, error = %e, "failed to send approved media");
                        }
                        self.sessions.set(tg_chat_id, ctx);
                        match response {
                            Ok(AgentResponse::Text(text)) => {
                                send_response_text(&bot, message.chat().id, &text).await?;
                            }
                            Ok(AgentResponse::ApprovalRequired { pending }) => {
                                bot.send_message(
                                    message.chat().id,
                                    format_approval_prompt(&pending),
                                )
                                .parse_mode(ParseMode::Html)
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
                if self.sessions.has_pending_subagent(tg_chat_id) {
                    let _ = self.sessions.take_pending_subagent(tg_chat_id);
                    bot.send_message(message.chat().id, "Subagent creation denied.")
                        .await?;
                } else if let Some(mut ctx) = self.sessions.get(tg_chat_id) {
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
            "subagents:active" => {
                handled = true;
                delete_callback_message = false;
                self.edit_subagents_menu(
                    &bot,
                    message.chat().id,
                    message.id(),
                    SubagentMenuView::Active,
                )
                .await?;
            }
            "subagents:recent" => {
                handled = true;
                delete_callback_message = false;
                self.edit_subagents_menu(
                    &bot,
                    message.chat().id,
                    message.id(),
                    SubagentMenuView::Recent,
                )
                .await?;
            }
            d if d.starts_with("subagent:show:") => {
                handled = true;
                delete_callback_message = false;
                let id = d.trim_start_matches("subagent:show:");
                self.edit_subagent_detail(&bot, message.chat().id, message.id(), id)
                    .await?;
            }
            d if d.starts_with("subagent:stop:") => {
                handled = true;
                delete_callback_message = false;
                let id = d.trim_start_matches("subagent:stop:");
                match self.subagents.stop_subagent(id).await {
                    Ok(()) => {
                        self.edit_subagent_detail(&bot, message.chat().id, message.id(), id)
                            .await?;
                    }
                    Err(e) => {
                        bot.send_message(
                            message.chat().id,
                            format!("Failed to stop subagent {}: {}", id, e),
                        )
                        .await?;
                    }
                }
            }
            "subagents:back_active" => {
                handled = true;
                delete_callback_message = false;
                self.edit_subagents_menu(
                    &bot,
                    message.chat().id,
                    message.id(),
                    SubagentMenuView::Active,
                )
                .await?;
            }
            "subagents:back_recent" => {
                handled = true;
                delete_callback_message = false;
                self.edit_subagents_menu(
                    &bot,
                    message.chat().id,
                    message.id(),
                    SubagentMenuView::Recent,
                )
                .await?;
            }
            _ => {}
        }

        if handled {
            if delete_callback_message
                && let Some(msg_ref) = q.message.as_ref()
            {
                let _ = bot.delete_message(msg_ref.chat().id, msg_ref.id()).await;
            }
            let _ = bot.answer_callback_query(q.id).await;
        }
        Ok(())
    }

    async fn send_subagents_menu(
        &self,
        bot: &Bot,
        chat: teloxide::types::ChatId,
        view: SubagentMenuView,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (text, keyboard) = self.render_subagents_menu(view).await;
        bot.send_message(chat, text)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await?;
        Ok(())
    }

    async fn edit_subagents_menu(
        &self,
        bot: &Bot,
        chat: teloxide::types::ChatId,
        message_id: teloxide::types::MessageId,
        view: SubagentMenuView,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (text, keyboard) = self.render_subagents_menu(view).await;
        let _ = bot
            .edit_message_text(chat, message_id, text)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await;
        Ok(())
    }

    async fn render_subagents_menu(
        &self,
        view: SubagentMenuView,
    ) -> (String, InlineKeyboardMarkup) {
        let cfg = &self.config.agent;
        let rows = self
            .load_subagents()
            .await
            .into_iter()
            .filter(|s| s.depth == 1)
            .collect::<Vec<_>>();
        let mut active = Vec::new();
        let mut recent = Vec::new();
        for s in rows {
            match s.status.as_str() {
                "pending" | "running" => active.push(s),
                _ => recent.push(s),
            }
        }
        let selected = match view {
            SubagentMenuView::Active => &active,
            SubagentMenuView::Recent => &recent,
        };
        let title = match view {
            SubagentMenuView::Active => "<b>Subagents — Active</b>",
            SubagentMenuView::Recent => "<b>Subagents — Recent</b>",
        };
        let mut text = format!(
            "{}\nDepth: <code>{}</code>\nConcurrent: <code>{}</code>\nToken budget: <code>{}</code>\nChild limit: <code>{}</code>\n",
            title, cfg.max_subagent_depth, cfg.max_concurrent_subagents, cfg.subagent_token_budget
            , cfg.max_child_subagents_per_parent
        );
        if selected.is_empty() {
            text.push_str("\nNo subagents in this view.");
        } else {
            text.push_str("\nSelect a parent subagent:");
            for s in selected.iter().take(4) {
                text.push_str(&format!("\n• {}", format_subagent_row_html(s)));
            }
        }

        let mut rows_buttons = vec![vec![
            InlineKeyboardButton::callback("Active", "subagents:active"),
            InlineKeyboardButton::callback("Recent", "subagents:recent"),
        ]];
        for s in selected.iter().take(4) {
            let short_id = s.id.chars().take(8).collect::<String>();
            let label = format!("{} [{}]", short_id, s.status);
            rows_buttons.push(vec![InlineKeyboardButton::callback(
                label,
                format!("subagent:show:{}", s.id),
            )]);
        }
        (text, InlineKeyboardMarkup::new(rows_buttons))
    }

    async fn edit_subagent_detail(
        &self,
        bot: &Bot,
        chat: teloxide::types::ChatId,
        message_id: teloxide::types::MessageId,
        subagent_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let detail = self.load_subagent_detail(subagent_id).await;
        let (text, back_view) = if let Some(d) = detail {
            let tail = read_subagent_log_tail(subagent_id, 10);
            let descendants = self.load_descendant_subagents(subagent_id).await;
            let hb_view = heartbeat_status_view(subagent_id, d.updated_at.as_deref());
            let current = if d.status == "running" || d.status == "pending" {
                latest_subagent_progress(subagent_id)
                    .map(|p| format!("Current work: {}", p))
                    .unwrap_or_else(|| "Current work: task is in progress.".to_string())
            } else if d.status == "completed" {
                "Current work: completed.".to_string()
            } else {
                "Current work: stopped with failure/cancel state.".to_string()
            };
            let child_lines = if descendants.is_empty() {
                "No child subagents.".to_string()
            } else {
                descendants
                    .into_iter()
                    .map(|s| format!("• {}", format_subagent_row_html(&s)))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let body = format!(
                "<b>Subagent</b> <code>{}</code>\nStatus: <code>{}</code>\nDepth: <code>{}</code>\nModel: <code>{}</code>\nUpdated: <code>{}</code>\nHeartbeat: <code>{}</code>\nTask: {}\n{}\n\n<b>Child subagents</b>\n{}\n\n<b>Result</b>\n<pre>{}</pre>\n\n<b>Recent actions</b>\n<pre>{}</pre>",
                escape_html(&d.id),
                escape_html(&d.status),
                d.depth,
                escape_html(&d.model.unwrap_or_else(|| "default".to_string())),
                escape_html(&d.updated_at.unwrap_or_else(|| "-".to_string())),
                escape_html(&hb_view),
                escape_html(&d.goal.unwrap_or_else(|| "-".to_string())),
                escape_html(&current),
                child_lines,
                escape_html(
                    &d.result
                        .as_deref()
                        .map(|r| truncate_for_message(r, 1800))
                        .unwrap_or_else(|| "No result yet.".to_string()),
                ),
                escape_html(if tail.is_empty() { "No log entries yet." } else { &tail })
            );
            let back = if d.status == "running" || d.status == "pending" {
                SubagentMenuView::Active
            } else {
                SubagentMenuView::Recent
            };
            (body, back)
        } else {
            (
                format!("Subagent {} not found.", subagent_id),
                SubagentMenuView::Active,
            )
        };
        let back_cb = match back_view {
            SubagentMenuView::Active => "subagents:back_active",
            SubagentMenuView::Recent => "subagents:back_recent",
        };
        let mut controls = vec![InlineKeyboardButton::callback("Back", back_cb)];
        if let Some(d) = self.load_subagent_detail(subagent_id).await
            && (d.status == "running" || d.status == "pending")
        {
            controls.push(InlineKeyboardButton::callback(
                "Stop",
                format!("subagent:stop:{}", subagent_id),
            ));
        }
        let keyboard = InlineKeyboardMarkup::new(vec![controls]);
        let _ = bot
            .edit_message_text(chat, message_id, text)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await;
        Ok(())
    }

    async fn load_subagent_detail(&self, subagent_id: &str) -> Option<SubagentDetailRow> {
        let sql = format!(
            "SELECT id, status, depth, goal, model, updated_at, result \
             FROM subagents WHERE id='{}' LIMIT 1",
            escape_sql(subagent_id)
        );
        let stmt = Statement::from_string(
            match self.db.db_type() {
                DbType::Postgres => DatabaseBackend::Postgres,
                DbType::Sqlite => DatabaseBackend::Sqlite,
            },
            sql,
        );
        let row = self.db.conn().query_one(stmt).await.ok().flatten()?;
        Some(SubagentDetailRow {
            id: row.try_get("", "id").ok()?,
            status: row.try_get("", "status").ok()?,
            depth: row.try_get("", "depth").unwrap_or_default(),
            goal: row.try_get("", "goal").ok(),
            model: row.try_get("", "model").ok(),
            updated_at: row.try_get("", "updated_at").ok(),
            result: row.try_get("", "result").ok(),
        })
    }

    async fn load_descendant_subagents(&self, root_id: &str) -> Vec<SubagentStatusRow> {
        let sql = format!(
            "SELECT id, status, depth, goal, model, updated_at, parent_agent_id \
             FROM subagents WHERE parent_agent_id='{}' ORDER BY updated_at DESC LIMIT 20",
            escape_sql(root_id)
        );
        let stmt = Statement::from_string(
            match self.db.db_type() {
                DbType::Postgres => DatabaseBackend::Postgres,
                DbType::Sqlite => DatabaseBackend::Sqlite,
            },
            sql,
        );
        match self.db.conn().query_all(stmt).await {
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
                    goal: r.try_get("", "goal").ok(),
                    model: r.try_get("", "model").ok(),
                    updated_at: r.try_get("", "updated_at").ok(),
                })
                .collect(),
            Err(_) => Vec::new(),
        }
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
        let prompt = "Generate a short and neutral first-time setup message (2-3 short sentences). \
Do not be overly friendly. \
Ask the user to specify how you should communicate (tone, brevity/detail, language) and what tasks to prioritize. \
End with wording similar to: Let me know when you are finished. \
Return plain text only, no markdown formatting.";
        match self.runtime.process(&mut ctx, prompt).await {
            Ok(AgentResponse::Text(t)) if !t.trim().is_empty() => t,
            _ => "Initial setup.\nTell me how I should communicate (tone, brevity/detail, language) and what tasks to prioritize.\nLet me know when you're finished."
                .to_string(),
        }
    }

    async fn generate_start_message(&self, tg_user_id: i64) -> String {
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
        let prompt =
            "Generate a short greeting message (1 sentence) for the beginning of a new chat. \
Keep it calm and professional. \
Do not use markdown, lists, or emojis. \
Do not mention settings or technical details. \
Return plain text only.";
        match self.runtime.process(&mut ctx, prompt).await {
            Ok(AgentResponse::Text(t)) if !t.trim().is_empty() => t,
            _ => "Hello. How can I help?".to_string(),
        }
    }

    async fn should_finalize_boot(&self, tg_user_id: i64, user_text: &str) -> bool {
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
        let prompt = format!(
            "Classify whether the user is explicitly asking to finish or exit BOOT configuration now.\n\
Return exactly one token: YES or NO.\n\
User message:\n{}",
            user_text
        );
        match self.runtime.process(&mut ctx, prompt).await {
            Ok(AgentResponse::Text(t)) => t.trim().eq_ignore_ascii_case("YES"),
            _ => false,
        }
    }

    async fn generate_boot_done_message(&self, tg_user_id: i64) -> String {
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
        let prompt = "Generate a short confirmation message (1-2 sentences) that onboarding configuration is complete and you are ready for work. \
Do not mention files, BOOT.md, memory processing, or technical internals. \
Tone: confident, concise, friendly. Plain text only.";
        match self.runtime.process(&mut ctx, prompt).await {
            Ok(AgentResponse::Text(t)) if !t.trim().is_empty() => t,
            _ => "Configuration is complete. I'm ready to work.".to_string(),
        }
    }

    async fn build_boot_profile_update(&self, tg_user_id: i64) -> Option<String> {
        let boot_raw = std::fs::read_to_string(workspace::boot_path()).ok()?;
        if boot_raw.trim().is_empty() {
            return None;
        }
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
        let prompt = format!(
            "You are processing first-run onboarding notes.\n\
Read the BOOT transcript and extract durable user preferences for long-term behavior.\n\
Return only concise markdown bullet points suitable for a `## User Preferences` section.\n\
No intro text, no code blocks, no technical details.\n\n\
BOOT transcript:\n{}",
            boot_raw
        );
        match self.runtime.process(&mut ctx, prompt).await {
            Ok(AgentResponse::Text(t)) if !t.trim().is_empty() => Some(t),
            _ => None,
        }
    }

    async fn approve_pending_subagent(
        &self,
        bot: &Bot,
        chat: teloxide::types::ChatId,
        tg_chat_id: i64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pending = match self.sessions.take_pending_subagent(tg_chat_id) {
            Some(p) => p,
            None => {
                bot.send_message(chat, "No pending subagent creation.")
                    .await?;
                return Ok(());
            }
        };
        let ctx = match self.sessions.get(tg_chat_id) {
            Some(c) => c,
            None => {
                let chat_id = ChatId::new();
                let new_ctx = AgentContext::new(
                    chat_id,
                    None,
                    unly_core::permissions::PermissionSet::admin(),
                    self.provider_registry
                        .default_provider()
                        .map(|p| p.name().to_string())
                        .unwrap_or_else(|_| "copilot".to_string()),
                    self.provider_registry.default_model(),
                    self.runtime.config().system_prompt.clone(),
                );
                self.sessions.set(tg_chat_id, new_ctx.clone());
                new_ctx
            }
        };

        let request = SubagentRequest {
            goal: pending.goal.clone(),
            parent_agent_id: pending.parent_agent_id,
            depth: pending.depth,
            permissions: unly_core::permissions::PermissionSet::admin(),
            provider: Some(pending.provider),
            model: Some(pending.model),
            token_budget: self.config.agent.subagent_token_budget,
        };

        match self
            .subagents
            .spawn_background(request, self.runtime.clone(), ctx.chat_id)
            .await
        {
            Ok(handle) => {
                bot.send_message(
                    chat,
                    format!(
                        "Subagent spawned with task: {}",
                        shorten_goal(&pending.goal)
                    ),
                )
                .await?;
                let bot_clone = bot.clone();
                let subagents = self.subagents.clone();
                let id = handle.id.to_string();
                tokio::spawn(async move {
                    wait_and_notify_subagent_result(&bot_clone, &subagents, chat, id).await;
                });
            }
            Err(e) => {
                bot.send_message(chat, format!("Failed to spawn subagent: {}", e))
                    .await?;
            }
        }
        Ok(())
    }
}

fn is_affirmative_approval(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    matches!(
        t.as_str(),
        "yes"
            | "ye"
            | "y"
            | "ok"
            | "okay"
            | "sure"
            | "confirm"
            | "approve"
            | "да"
            | "ок"
            | "ага"
            | "подтверждаю"
    )
}

fn is_negative_approval(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    matches!(
        t.as_str(),
        "no" | "n" | "deny" | "not now" | "cancel" | "нет" | "не" | "отмена"
    )
}

/// Maximum characters shown in tool argument previews inside approval messages.
const APPROVAL_PREVIEW_MAX_CHARS: usize = 80;
/// Maximum characters shown in HTTP body previews inside approval messages.
const APPROVAL_BODY_PREVIEW_MAX_CHARS: usize = 100;

fn format_pending_approvals(pending: &[unly_agent::context::PendingApproval]) -> String {
    pending
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let risk_label = match p.risk_level.as_str() {
                "Dangerous" => "dangerous (destructive or irreversible)",
                "Privileged" => "privileged (mutating or external action)",
                _ => "safe (read-only)",
            };

            let detail = build_approval_detail(&p.tool_name, &p.args);
            format!(
                "<b>{}. {} ({})</b>\n{}",
                i + 1,
                escape_html(&p.tool_name),
                risk_label,
                detail
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Build a human-readable, contextual description of what a tool call will do.
fn build_approval_detail(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "bash" | "shell" => {
            if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("run");
                let mode_label = match mode {
                    "start" => " (background)",
                    "status" => " (check status)",
                    _ => "",
                };
                format!(
                    "  Action: Execute shell command{}\n  Command: <code>{}</code>",
                    mode_label,
                    escape_html(cmd)
                )
            } else {
                "  Action: Execute shell command (no command provided)".to_string()
            }
        }
        "fs_write" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown path>");
            let append = args
                .get("append")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let action = if append { "Append to" } else { "Overwrite" };
            let content_preview = args
                .get("content")
                .and_then(|v| v.as_str())
                .map(|c| {
                    let preview = c
                        .chars()
                        .take(APPROVAL_PREVIEW_MAX_CHARS)
                        .collect::<String>();
                    if c.len() > APPROVAL_PREVIEW_MAX_CHARS {
                        format!("{}…", preview)
                    } else {
                        preview
                    }
                })
                .unwrap_or_default();
            format!(
                "  Action: {} file\n  Path: <code>{}</code>\n  Preview: <code>{}</code>",
                action,
                escape_html(path),
                escape_html(&content_preview)
            )
        }
        "fs_delete" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown path>");
            let recursive = args
                .get("recursive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let scope = if recursive {
                "recursively (directory + all contents)"
            } else {
                "single file or empty directory"
            };
            format!(
                "  Action: Delete {}\n  Target: <code>{}</code>",
                scope,
                escape_html(path)
            )
        }
        "fs_copy" => {
            let src = args
                .get("src")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let dst = args
                .get("dst")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            format!(
                "  Action: Copy file\n  From: <code>{}</code>\n  To: <code>{}</code>",
                escape_html(src),
                escape_html(dst)
            )
        }
        "fs_move" => {
            let src = args
                .get("src")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let dst = args
                .get("dst")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            format!(
                "  Action: Move or rename\n  From: <code>{}</code>\n  To: <code>{}</code>",
                escape_html(src),
                escape_html(dst)
            )
        }
        "http_post" => {
            let url = args
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown url>");
            let body_preview = args
                .get("body")
                .map(|b| {
                    let s = b.to_string();
                    let preview = s
                        .chars()
                        .take(APPROVAL_BODY_PREVIEW_MAX_CHARS)
                        .collect::<String>();
                    if s.len() > APPROVAL_BODY_PREVIEW_MAX_CHARS {
                        format!("{}…", preview)
                    } else {
                        preview
                    }
                })
                .unwrap_or_default();
            format!(
                "  Action: Send HTTP POST request\n  URL: <code>{}</code>\n  Body: <code>{}</code>",
                escape_html(url),
                escape_html(&body_preview)
            )
        }
        "spawn_subagent" => {
            let task = args
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or("<no task>");
            let model = args
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            format!(
                "  Action: Start background subagent\n  Task: {}\n  Model: {}",
                escape_html(task),
                escape_html(model)
            )
        }
        "cron_job" => {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let cron = args
                .get("cron_expression")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match action {
                "create" => format!(
                    "  Action: Create scheduled cron job\n  Name: {}\n  Schedule: <code>{}</code>\n  Task: {}",
                    escape_html(name),
                    escape_html(cron),
                    escape_html(task)
                ),
                "delete" => format!(
                    "  Action: Delete scheduled cron job\n  Job ID: <code>{}</code>",
                    escape_html(id)
                ),
                _ => format!(
                    "  Action: {} cron job\n  Job ID: <code>{}</code>",
                    escape_html(action),
                    escape_html(id)
                ),
            }
        }
        _ => {
            // Generic: show all non-internal args as key: value pairs.
            let pairs: Vec<String> = args
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .filter(|(k, _)| !k.starts_with("__"))
                        .map(|(k, v)| {
                            let val = match v {
                                serde_json::Value::String(s) => {
                                    let preview = s
                                        .chars()
                                        .take(APPROVAL_PREVIEW_MAX_CHARS)
                                        .collect::<String>();
                                    if s.len() > APPROVAL_PREVIEW_MAX_CHARS {
                                        format!("{}…", preview)
                                    } else {
                                        preview
                                    }
                                }
                                other => other.to_string(),
                            };
                            format!("  {}: <code>{}</code>", escape_html(k), escape_html(&val))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if pairs.is_empty() {
                "  (no arguments)".to_string()
            } else {
                pairs.join("\n")
            }
        }
    }
}

/// Build the complete approval prompt text shown to the user.
fn format_approval_prompt(pending: &[unly_agent::context::PendingApproval]) -> String {
    let count = pending.len();
    let header = if count == 1 {
        "<b>Approval required</b>\n\nPending action:".to_string()
    } else {
        format!(
            "<b>Approval required</b>\n\nPending actions: <b>{}</b>",
            count
        )
    };
    let details = format_pending_approvals(pending);
    format!(
        "{}\n\n{}\n\nUse Approve or Deny to continue.",
        header, details
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
            // Skip an optional language hint only if a newline appears before
            // the closing fence. Otherwise, treat same-line content as code.
            let mut j = i;
            let mut has_language_hint = false;
            while j < n {
                if j + 2 < n && chars[j] == '`' && chars[j + 1] == '`' && chars[j + 2] == '`' {
                    break;
                }
                if chars[j] == '\n' {
                    has_language_hint = true;
                    break;
                }
                j += 1;
            }
            if has_language_hint {
                i = j + 1; // consume the newline after the language hint
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

        // ── Markdown headers #/##/### (Telegram has no h1-h6) ────────────────
        if (i == 0 || chars[i - 1] == '\n') && chars[i] == '#' {
            let mut j = i;
            while j < n && chars[j] == '#' {
                j += 1;
            }
            let level = j - i;
            if (1..=3).contains(&level) && j < n && chars[j] == ' ' {
                let text_start = j + 1;
                let text_end = chars[text_start..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map(|p| text_start + p)
                    .unwrap_or(n);
                let header_text: String = chars[text_start..text_end].iter().collect();
                out.push_str("<b>");
                push_html_escaped(&mut out, header_text.trim());
                out.push_str("</b>\n\n");
                i = text_end;
                if i < n && chars[i] == '\n' {
                    i += 1;
                }
                continue;
            }
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
            if let Some(j) = chars[start..].iter().position(|&c| c == '*')
                && j > 0
            {
                let txt: String = chars[start..start + j].iter().collect();
                out.push_str("<i>");
                push_html_escaped(&mut out, &txt);
                out.push_str("</i>");
                i = start + j + 1;
                continue;
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
            } else if matches!(
                rest.get(..4),
                Some(['&', 'l', 't', ';'] | ['&', 'g', 't', ';'])
            ) {
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

fn parse_telegram_tag_parts(s: &str) -> Option<(bool, String, &str)> {
    let inner = s.strip_prefix('<')?.strip_suffix('>')?.trim();
    let (is_closing, inner) = if let Some(rest) = inner.strip_prefix('/') {
        (true, rest.trim_start())
    } else {
        (false, inner)
    };

    let tag_name: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();

    if tag_name.is_empty() {
        return None;
    }

    let remainder = &inner[tag_name.len()..];
    Some((is_closing, tag_name.to_lowercase(), remainder))
}

fn is_single_quoted_attr(remainder: &str, attr_name: &str) -> Option<String> {
    let rest = remainder.trim();
    let rest = rest.strip_prefix(attr_name)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();

    let quote = if let Some(rest) = rest.strip_prefix('"') {
        ('"', rest)
    } else if let Some(rest) = rest.strip_prefix('\'') {
        ('\'', rest)
    } else {
        return None;
    };

    let (quote_char, rest) = quote;
    let end = rest.find(quote_char)?;
    let value = &rest[..end];
    let trailing = rest[end + quote_char.len_utf8()..].trim();

    if trailing.is_empty() {
        Some(value.to_string())
    } else {
        None
    }
}

fn is_single_boolean_attr(remainder: &str, attr_name: &str) -> bool {
    remainder.trim() == attr_name
}

/// Return true if `s` is a Telegram-supported HTML tag with only
/// Telegram-supported attributes, so it can be passed through unchanged.
fn is_telegram_html_tag(s: &str) -> bool {
    let Some((is_closing, tag_name, remainder)) = parse_telegram_tag_parts(s) else {
        return false;
    };

    if is_closing {
        return remainder.trim().is_empty()
            && matches!(
                tag_name.as_str(),
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
            );
    }

    match tag_name.as_str() {
        "b" | "strong" | "i" | "em" | "u" | "ins" | "s" | "strike" | "del" | "pre"
        | "tg-spoiler" => remainder.trim().is_empty(),
        "a" => is_single_quoted_attr(remainder, "href")
            .map(|href| !href.is_empty())
            .unwrap_or(false),
        "tg-emoji" => is_single_quoted_attr(remainder, "emoji-id")
            .map(|emoji_id| !emoji_id.is_empty() && emoji_id.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false),
        "code" => {
            let trimmed = remainder.trim();
            trimmed.is_empty()
                || is_single_quoted_attr(remainder, "class")
                    .map(|class| class.starts_with("language-") && class.len() > "language-".len())
                    .unwrap_or(false)
        }
        "blockquote" => {
            let trimmed = remainder.trim();
            trimmed.is_empty() || is_single_boolean_attr(remainder, "expandable")
        }
        _ => false,
    }
}
/// Split raw (pre-conversion) text at natural boundaries so that each chunk
/// can be independently converted to Telegram HTML.
///
/// Tries to split at `\n\n` (paragraph break), then at `\n` (line break),
/// and as a last resort at a safe UTF-8 character boundary near `max_len`.
/// Because splitting happens on the raw text (before HTML conversion), no HTML
/// tag can ever straddle a chunk boundary.
fn split_at_boundary(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while remaining.len() > max_len {
        // Find the largest UTF-8 char boundary that fits within max_len bytes.
        let safe_end = (0..=max_len.min(remaining.len()))
            .rev()
            .find(|&i| remaining.is_char_boundary(i))
            .unwrap_or(0);

        if safe_end == 0 {
            break; // Pathological input – give up rather than loop forever.
        }

        let window = &remaining[..safe_end];

        // Prefer a paragraph break, then a line break, then a hard cut.
        let split_pos = window
            .rfind("\n\n")
            .or_else(|| window.rfind('\n'))
            .unwrap_or(safe_end);

        if split_pos == 0 {
            // No newline found within the window – hard cut at the safe boundary.
            chunks.push(remaining[..safe_end].to_string());
            remaining = &remaining[safe_end..];
        } else {
            chunks.push(remaining[..split_pos].to_string());
            remaining = remaining[split_pos..].trim_start_matches('\n');
        }
    }

    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }

    chunks
}

#[derive(Debug, Clone)]
struct SubagentStatusRow {
    id: String,
    status: String,
    depth: i32,
    goal: Option<String>,
    model: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum SubagentMenuView {
    Active,
    Recent,
}

#[derive(Debug, Clone)]
struct SubagentDetailRow {
    id: String,
    status: String,
    depth: i32,
    goal: Option<String>,
    model: Option<String>,
    updated_at: Option<String>,
    result: Option<String>,
}

fn format_subagent_row_html(s: &SubagentStatusRow) -> String {
    let model = s.model.as_deref().unwrap_or("default");
    let freshness = s
        .updated_at
        .as_deref()
        .map(heartbeat_freshness)
        .unwrap_or("unknown".to_string());
    let goal = s
        .goal
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ");
    let updated = s
        .updated_at
        .as_deref()
        .map(short_ts)
        .unwrap_or_else(|| "-".to_string());
    format!(
        "<code>{}</code> [{}] d={} model={} at={} hb={} {}",
        escape_html(&s.id),
        escape_html(&s.status),
        s.depth,
        escape_html(model),
        escape_html(&updated),
        escape_html(&freshness),
        if goal.is_empty() {
            "".to_string()
        } else {
            format!("task: {}", escape_html(&goal))
        }
    )
}

fn read_subagent_log_tail(subagent_id: &str, lines: usize) -> String {
    let path = unly_config::workspace::subagent_logs_dir().join(format!("{}.log", subagent_id));
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut collected = content
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(lines)
        .collect::<Vec<_>>();
    collected.reverse();
    collected.join("\n")
}

fn latest_subagent_progress(subagent_id: &str) -> Option<String> {
    let path = unly_config::workspace::subagent_logs_dir().join(format!("{}.log", subagent_id));
    let Ok(content) = std::fs::read_to_string(path) else {
        return None;
    };
    let blocks: Vec<&str> = content.split("\n\n").collect();
    for block in blocks.into_iter().rev() {
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("progress=") {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        if !block.contains("status=progress") {
            continue;
        }
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("result=") {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn heartbeat_status_view(subagent_id: &str, updated_at: Option<&str>) -> String {
    let freshness = updated_at
        .map(heartbeat_freshness)
        .unwrap_or_else(|| "unknown".to_string());
    let path = unly_config::workspace::subagent_logs_dir().join(format!("{}.log", subagent_id));
    let Ok(content) = std::fs::read_to_string(path) else {
        return format!("{} (no logs)", freshness);
    };
    let mut total_hb = 0u64;
    let mut with_progress = 0u64;
    for line in content.lines() {
        if line.contains("status=heartbeat") {
            total_hb += 1;
        }
        if line.starts_with("result=") && line.contains("elapsed=") && line.contains("tick=") {
            with_progress += 1;
        }
    }
    if total_hb == 0 {
        return format!("{} (no heartbeat entries)", freshness);
    }
    if with_progress == 0 {
        return format!("{} (heartbeat-only, no progress snapshot)", freshness);
    }
    format!(
        "{} (progress snapshots: {}/{})",
        freshness, with_progress, total_hb
    )
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}

fn short_ts(ts: &str) -> String {
    ts.get(0..19).unwrap_or(ts).replace('T', " ")
}

fn heartbeat_freshness(updated_at: &str) -> String {
    let Ok(ts) = chrono::DateTime::parse_from_rfc3339(updated_at) else {
        return "unknown".to_string();
    };
    let age = chrono::Utc::now().signed_duration_since(ts.with_timezone(&chrono::Utc));
    let secs = age.num_seconds();
    if secs <= 30 {
        "alive".to_string()
    } else if secs <= 90 {
        "slow".to_string()
    } else {
        "stale".to_string()
    }
}

async fn send_response_text(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    text: &str,
) -> Result<(), teloxide::RequestError> {
    if text.is_empty() {
        return Ok(());
    }

    // Split the *raw* text before HTML conversion so that each chunk is
    // independently converted and yields self-contained valid HTML.
    // HTML conversion can expand text (e.g. `&` → `&amp;`), so use a
    // conservative raw-text limit of 3600 characters to stay safely under
    // Telegram's 4096-character HTML message limit.
    for raw_chunk in split_at_boundary(text, 3600) {
        if !raw_chunk.is_empty() {
            let html = convert_to_telegram_html(&raw_chunk);
            send_message_formatted(bot, chat_id, html).await?;
        }
    }
    Ok(())
}

async fn send_media(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    kind: &MediaKind,
    path: &str,
    caption: Option<&str>,
) -> Result<(), teloxide::RequestError> {
    let input = InputFile::file(path.to_string());
    match kind {
        MediaKind::Photo => {
            let req = bot.send_photo(chat_id, input);
            if let Some(caption) = caption {
                req.caption(caption.to_string()).await?;
            } else {
                req.await?;
            }
        }
        MediaKind::Document => {
            let req = bot.send_document(chat_id, input);
            if let Some(caption) = caption {
                req.caption(caption.to_string()).await?;
            } else {
                req.await?;
            }
        }
    }
    Ok(())
}

async fn drain_pending_media(
    bot: &Bot,
    chat_id: teloxide::types::ChatId,
    ctx: &mut AgentContext,
) -> Result<(), teloxide::RequestError> {
    let pending = std::mem::take(&mut ctx.pending_media);
    for media in pending {
        send_media(
            bot,
            chat_id,
            &media.kind,
            &media.path,
            media.caption.as_deref(),
        )
        .await?;
    }
    Ok(())
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

fn user_input_for_storage(input: &ChatMessageContent) -> String {
    match input {
        ChatMessageContent::Text(text) => text.clone(),
        ChatMessageContent::Parts(parts) => {
            let mut out = Vec::new();
            for part in parts {
                match part {
                    ContentPart::Text { text } => out.push(text.clone()),
                    ContentPart::ImageUrl { .. } => out.push("[image attached]".to_string()),
                }
            }
            out.join("\n")
        }
    }
}

fn parse_spawn_subagent_request(text: &str) -> Option<String> {
    let prefix = "/spawn_subagent";
    if !text.starts_with(prefix) {
        return None;
    }
    let goal = text[prefix.len()..].trim();
    if goal.is_empty() {
        None
    } else {
        Some(goal.to_string())
    }
}

fn shorten_goal(goal: &str) -> String {
    let max = 120usize;
    if goal.chars().count() <= max {
        return goal.to_string();
    }
    goal.chars().take(max).collect::<String>() + "…"
}

async fn wait_and_notify_subagent_result(
    bot: &Bot,
    manager: &SubagentManager,
    chat_id: teloxide::types::ChatId,
    subagent_id: String,
) {
    for _ in 0..180u32 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let status = manager.subagent_status(&subagent_id).await;
        match status {
            Some(s) if s == "completed" => {
                let result_text = manager
                    .subagent_outcome(&subagent_id)
                    .await
                    .and_then(|(_, r)| r)
                    .unwrap_or_else(|| "No result payload.".to_string());
                let _ = bot
                    .send_message(
                        chat_id,
                        format!(
                            "Subagent {} completed.\nResult:\n{}",
                            subagent_id,
                            truncate_for_message(&result_text, 3200)
                        ),
                    )
                    .await;
                return;
            }
            Some(s) if s == "failed" => {
                let _ = bot
                    .send_message(chat_id, format!("Subagent {} failed.", subagent_id))
                    .await;
                return;
            }
            _ => {}
        }
    }
}

fn truncate_for_message(text: &str, max_chars: usize) -> String {
    let mut out = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

async fn resolve_telegram_chat_id(db: &Database, chat_id: ChatId) -> Option<i64> {
    let sql = format!(
        "SELECT telegram_chat_id FROM chats WHERE id='{}' LIMIT 1",
        chat_id
    );
    let stmt = Statement::from_string(
        match db.db_type() {
            DbType::Postgres => DatabaseBackend::Postgres,
            DbType::Sqlite => DatabaseBackend::Sqlite,
        },
        sql,
    );
    let row = db.conn().query_one(stmt).await.ok().flatten()?;
    row.try_get("", "telegram_chat_id").ok()
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

    #[test]
    fn test_split_then_convert_no_broken_html() {
        // A long message with **bold** spanning an otherwise natural split boundary.
        // Splitting raw text first, then converting each chunk, should produce
        // balanced HTML per chunk (the bold markers stay within one chunk).
        let long_line = "word ".repeat(20); // 100 chars
        let bold_span = format!("{}**important**{}", long_line, long_line);
        let chunks: Vec<String> = split_at_boundary(&bold_span, 120)
            .into_iter()
            .map(|raw| convert_to_telegram_html(&raw))
            .collect();
        // Every chunk must have balanced <b>...</b> tags (no orphan opening tag).
        for chunk in &chunks {
            let opens = chunk.matches("<b>").count();
            let closes = chunk.matches("</b>").count();
            assert_eq!(opens, closes, "unbalanced <b> in chunk: {}", chunk);
        }
    }

    #[test]
    fn test_convert_markdown_header_h3_to_bold_with_spacing() {
        let input = "### Setup\nDo this";
        let html = convert_to_telegram_html(input);
        assert_eq!(html, "<b>Setup</b>\n\nDo this");
    }
}
