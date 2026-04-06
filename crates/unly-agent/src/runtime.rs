use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use unly_config::AppConfig;
use unly_core::{
    Result,
    model::{ChatMessage, ChatMessageContent, ChatRequest, ContentPart, StreamChunk},
    permissions::Permission,
    provider::Provider,
    tool::ToolContext,
};
use unly_memory::{MemoryQuery, MemoryScope, MemoryStore};
use unly_plugins::{PluginLoader, SkillLoader};
use unly_tools::ToolRegistry;

use crate::context::{AgentContext, MediaKind, MediaSend, PendingApproval};

/// Configuration for the agent runtime.
pub struct AgentRuntimeConfig {
    pub system_prompt: String,
    pub default_provider: String,
    pub default_model: String,
    pub max_tool_calls_per_turn: u32,
    pub max_turns: u32,
    pub context_window_size: usize,
    pub inject_memory_context: bool,
    pub memory_context_top_k: usize,
    pub memory_context_similarity_threshold: f32,
    pub memory_context_max_chars_per_item: usize,
    pub memory_context_max_total_chars: usize,
    pub memory_store_conversation_turns: bool,
    pub memory_store_max_chars_per_turn: usize,
    pub use_file_memory_primary: bool,
    pub file_memory_index_path: String,
    pub file_memory_today_path: String,
    pub file_memory_max_chars_per_file: usize,
    pub file_memory_max_total_chars: usize,
    pub enable_db_memory_augmentation: bool,
    pub append_turns_to_today_memory: bool,
    pub force_plain_output: bool,
    pub app_config: Option<AppConfig>,
}

/// The main agent runtime.
///
/// Handles the agentic loop: receive message → plan → call tools → respond.
pub struct AgentRuntime {
    config: AgentRuntimeConfig,
    provider_registry: Arc<unly_providers::ProviderRegistry>,
    tool_registry: Arc<ToolRegistry>,
    audit: Option<Arc<unly_audit::AuditLogger>>,
    memory_store: Option<Arc<MemoryStore>>,
}

impl AgentRuntime {
    pub fn config(&self) -> &AgentRuntimeConfig {
        &self.config
    }

    /// Return the execution policy governing tool approval requirements.
    pub fn tool_policy(&self) -> &unly_tools::policy::ExecutionPolicy {
        self.tool_registry.policy()
    }

    fn build_system_prompt_with_hot_reload(&self) -> String {
        if let Some(app_config) = &self.config.app_config {
            let mut prompt = self.config.system_prompt.clone();
            prompt.push_str(&build_runtime_extensions_prompt(
                self.tool_registry.as_ref(),
                app_config,
            ));
            prompt
        } else {
            self.config.system_prompt.clone()
        }
    }

    pub fn new(
        config: AgentRuntimeConfig,
        provider_registry: Arc<unly_providers::ProviderRegistry>,
        tool_registry: Arc<ToolRegistry>,
        audit: Option<Arc<unly_audit::AuditLogger>>,
        memory_store: Option<Arc<MemoryStore>>,
    ) -> Self {
        Self {
            config,
            provider_registry,
            tool_registry,
            audit,
            memory_store,
        }
    }

    /// Process a user message in a given context, returning the assistant response.
    ///
    /// Runs the full agentic loop including tool calls.
    /// Returns the final text response (or an approval request if a privileged
    /// tool was requested without approval).
    pub async fn process(
        &self,
        ctx: &mut AgentContext,
        user_message: impl Into<String>,
    ) -> Result<AgentResponse> {
        self.process_input(ctx, ChatMessageContent::Text(user_message.into()))
            .await
    }

    pub async fn process_input(
        &self,
        ctx: &mut AgentContext,
        user_input: ChatMessageContent,
    ) -> Result<AgentResponse> {
        ctx.system_prompt = self.build_system_prompt_with_hot_reload();
        let user_msg = user_input_for_memory(&user_input);
        let mut memory_ctx = self.build_memory_context(ctx, &user_msg).await;
        let user_msg_for_memory = user_msg.clone();

        ctx.push_message(ChatMessage {
            role: "user".to_string(),
            content: user_input,
            tool_call_id: None,
            tool_calls: None,
            name: None,
        });

        ctx.turn_count += 1;
        if ctx.turn_count > self.config.max_turns {
            return Err(unly_core::Error::Agent("max turns exceeded".to_string()));
        }

        let tool_defs = self.build_tool_defs(ctx);
        let provider = self.get_provider(&ctx.provider)?;
        let mut loop_count = 0u32;
        let mut forced_tool_retry = false;
        let mut provider_retry_count = 0u32;
        let mut prompt_compression_round = 0u32;
        let preapproved_tools = matches!(ctx.tool_approval_override, Some(true))
            || (ctx.subagent_depth > 0 && ctx.permissions.has(&Permission::ExecutePrivilegedTools));
        let force_tool_approval = matches!(ctx.tool_approval_override, Some(false));

        let final_response = loop {
            ctx.trim_to(self.config.context_window_size);

            // --- THINKING PHASE: try to make a standard (non-streaming) call ---
            // Tool-call rounds are "thinking" — we use the non-streaming chat API.
            let request = ChatRequest {
                model: ctx.model.clone(),
                messages: self.build_request_messages(ctx, memory_ctx.as_deref()),
                tools: if tool_defs.is_empty() {
                    None
                } else {
                    Some(tool_defs.clone())
                },
                temperature: Some(0.7),
                max_tokens: Some(4096),
                stream: false,
            };
            if provider.capabilities().reasoning {
                ctx.log_thinking("reasoning_mode", "provider exposes reasoning channel");
            } else {
                ctx.log_thinking(
                    "reasoning_mode",
                    "provider has no explicit reasoning channel",
                );
            }

            let response = match provider.chat(request).await {
                Ok(resp) => {
                    provider_retry_count = 0;
                    resp
                }
                Err(e) => {
                    if let Some(limit) = parse_prompt_token_limit(&e)
                        && compress_context_for_token_limit(
                            ctx,
                            &mut memory_ctx,
                            limit,
                            prompt_compression_round,
                        )
                    {
                        prompt_compression_round += 1;
                        provider_retry_count = 0;
                        ctx.log_thinking(
                            "prompt_compression",
                            format!(
                                "prompt exceeded model limit ({} tokens); compressed context (round {})",
                                limit, prompt_compression_round
                            ),
                        );
                        continue;
                    }
                    provider_retry_count += 1;
                    if provider_retry_count <= 2 {
                        ctx.log_thinking(
                            "provider_error_retry",
                            format!("retry {} after provider error: {}", provider_retry_count, e),
                        );
                        continue;
                    }
                    return Err(e);
                }
            };

            if let Some(tool_calls) = response.tool_calls.as_ref().filter(|tc| !tc.is_empty()) {
                // --- THINKING PHASE: execute tool calls ---
                ctx.push_message(ChatMessage {
                    role: "assistant".to_string(),
                    content: ChatMessageContent::Text(response.content.clone().unwrap_or_default()),
                    tool_call_id: None,
                    tool_calls: Some(tool_calls.clone()),
                    name: None,
                });

                let mut pending_approval: Vec<PendingApproval> = Vec::new();
                let mut tool_limit_exceeded = false;

                for tc in tool_calls {
                    let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(serde_json::Value::Object(Default::default()));

                    let tool_ctx = ToolContext {
                        tool_call_id: tc.id.clone(),
                        user_id: ctx.user_id,
                        chat_id: Some(ctx.chat_id),
                        agent_id: Some(ctx.agent_id),
                        subagent_depth: ctx.subagent_depth,
                    };

                    loop_count += 1;
                    if loop_count > self.config.max_tool_calls_per_turn {
                        warn!("max tool calls per turn exceeded");
                        tool_limit_exceeded = true;
                        break;
                    }

                    // Log to thinking: what tool is being called.
                    ctx.log_thinking(
                        "tool_call",
                        format!(
                            "{}({})",
                            tc.function.name,
                            &tc.function.arguments[..tc.function.arguments.len().min(120)]
                        ),
                    );

                    let result = self
                        .tool_registry
                        .execute(
                            &tc.function.name,
                            args.clone(),
                            tool_ctx,
                            preapproved_tools,
                            force_tool_approval,
                        )
                        .await;

                    match result {
                        Ok(tool_result) => {
                            if let Some(audit) = &self.audit {
                                if tool_result.is_error {
                                    audit.failure(
                                        "tool_execution",
                                        tc.function.name.clone(),
                                        "execute",
                                        &tool_result.stderr,
                                    );
                                } else {
                                    audit.success(
                                        "tool_execution",
                                        tc.function.name.clone(),
                                        "execute",
                                    );
                                }
                            }

                            // Collect any queued Telegram media sends.
                            if !tool_result.is_error {
                                collect_media_from_result(ctx, &tool_result.metadata);
                            }

                            let content = if tool_result.is_error {
                                format!("Error: {}", tool_result.stderr)
                            } else {
                                tool_result.stdout.clone()
                            };

                            // Log result to thinking.
                            ctx.log_thinking(
                                "tool_result",
                                format!(
                                    "{}: {}",
                                    tc.function.name,
                                    truncate_to_chars(&content, 200)
                                ),
                            );

                            ctx.push_message(ChatMessage {
                                role: "tool".to_string(),
                                content: ChatMessageContent::Text(content),
                                tool_call_id: Some(tc.id.clone()),
                                tool_calls: None,
                                name: None,
                            });
                        }
                        Err(unly_core::Error::ToolDenied { reason }) => {
                            if let Some(audit) = &self.audit {
                                audit.denied(
                                    "tool_execution",
                                    tc.function.name.clone(),
                                    "execute",
                                    &reason,
                                );
                            }
                            pending_approval.push(PendingApproval {
                                tool_call_id: tc.id.clone(),
                                tool_name: tc.function.name.clone(),
                                args,
                                risk_level: "privileged".to_string(),
                            });
                        }
                        Err(e) => {
                            warn!(tool = %tc.function.name, error = %e, "tool error");
                            let err_msg = format!("Error: {}", e);
                            ctx.log_thinking("tool_error", format!("{}: {}", tc.function.name, e));
                            ctx.push_message(ChatMessage {
                                role: "tool".to_string(),
                                content: ChatMessageContent::Text(err_msg),
                                tool_call_id: Some(tc.id.clone()),
                                tool_calls: None,
                                name: None,
                            });
                        }
                    }
                }

                if !pending_approval.is_empty() {
                    ctx.pending_approvals = pending_approval.clone();
                    return Ok(AgentResponse::ApprovalRequired {
                        pending: pending_approval,
                    });
                }
                if tool_limit_exceeded {
                    return Err(unly_core::Error::Agent(
                        "max tool calls per turn exceeded".to_string(),
                    ));
                }

                continue;
            }

            // --- RESPONSE PHASE: no tool calls — this is the final answer ---
            let raw_text = response.content.unwrap_or_default();
            if !forced_tool_retry
                && !tool_defs.is_empty()
                && looks_like_manual_confirmation_request(&raw_text)
            {
                forced_tool_retry = true;
                ctx.push_message(ChatMessage {
                    role: "assistant".to_string(),
                    content: ChatMessageContent::Text(raw_text),
                    tool_call_id: None,
                    tool_calls: None,
                    name: None,
                });
                ctx.push_message(ChatMessage {
                    role: "system".to_string(),
                    content: ChatMessageContent::Text(
                        "Do not ask for permission in plain text. If a tool is required, call the tool now and let runtime handle approval via Approve/Deny."
                            .to_string(),
                    ),
                    tool_call_id: None,
                    tool_calls: None,
                    name: None,
                });
                continue;
            }
            // Strip <think>…</think> blocks from the user-visible response.
            let mut text = strip_thinking_tags(&raw_text);
            if self.config.force_plain_output {
                text = strip_html_tags(&text);
            }
            ctx.push_message(ChatMessage {
                role: "assistant".to_string(),
                content: ChatMessageContent::Text(raw_text),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            });
            break text;
        };

        self.store_memory_turn(ctx, &user_msg_for_memory, &final_response)
            .await;

        Ok(AgentResponse::Text(final_response))
    }

    /// Process the final response using streaming, sending tokens to `sender`.
    ///
    /// Tool-call rounds (thinking phase) are executed synchronously first;
    /// only the final text generation is streamed.
    pub async fn process_stream(
        &self,
        ctx: &mut AgentContext,
        user_message: impl Into<String>,
        sender: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        self.process_stream_input(ctx, ChatMessageContent::Text(user_message.into()), sender)
            .await
    }

    pub async fn process_stream_input(
        &self,
        ctx: &mut AgentContext,
        user_input: ChatMessageContent,
        sender: mpsc::Sender<StreamEvent>,
    ) -> Result<()> {
        ctx.system_prompt = self.build_system_prompt_with_hot_reload();
        let user_msg = user_input_for_memory(&user_input);
        let mut memory_ctx = self.build_memory_context(ctx, &user_msg).await;
        let user_msg_for_memory = user_msg.clone();

        ctx.push_message(ChatMessage {
            role: "user".to_string(),
            content: user_input,
            tool_call_id: None,
            tool_calls: None,
            name: None,
        });

        ctx.turn_count += 1;
        if ctx.turn_count > self.config.max_turns {
            return Err(unly_core::Error::Agent("max turns exceeded".to_string()));
        }

        let tool_defs = self.build_tool_defs(ctx);
        let provider = self.get_provider(&ctx.provider)?;
        let mut loop_count = 0u32;
        let mut forced_tool_retry = false;
        let mut provider_retry_count = 0u32;
        let mut prompt_compression_round = 0u32;
        let preapproved_tools = matches!(ctx.tool_approval_override, Some(true))
            || (ctx.subagent_depth > 0 && ctx.permissions.has(&Permission::ExecutePrivilegedTools));
        let force_tool_approval = matches!(ctx.tool_approval_override, Some(false));

        loop {
            ctx.trim_to(self.config.context_window_size);

            // Check if there are potential tool calls by doing a non-streaming call.
            // We only stream the FINAL response (when there are no more tools to call).
            let probe_request = ChatRequest {
                model: ctx.model.clone(),
                messages: self.build_request_messages(ctx, memory_ctx.as_deref()),
                tools: if tool_defs.is_empty() {
                    None
                } else {
                    Some(tool_defs.clone())
                },
                temperature: Some(0.7),
                max_tokens: Some(4096),
                stream: false,
            };
            if provider.capabilities().reasoning {
                ctx.log_thinking("reasoning_mode", "provider exposes reasoning channel");
            } else {
                ctx.log_thinking(
                    "reasoning_mode",
                    "provider has no explicit reasoning channel",
                );
            }

            let response = match provider.chat(probe_request).await {
                Ok(resp) => {
                    provider_retry_count = 0;
                    resp
                }
                Err(e) => {
                    if let Some(limit) = parse_prompt_token_limit(&e)
                        && compress_context_for_token_limit(
                            ctx,
                            &mut memory_ctx,
                            limit,
                            prompt_compression_round,
                        )
                    {
                        prompt_compression_round += 1;
                        provider_retry_count = 0;
                        ctx.log_thinking(
                            "prompt_compression",
                            format!(
                                "prompt exceeded model limit ({} tokens); compressed context (round {})",
                                limit, prompt_compression_round
                            ),
                        );
                        continue;
                    }
                    provider_retry_count += 1;
                    if provider_retry_count <= 2 {
                        ctx.log_thinking(
                            "provider_error_retry",
                            format!("retry {} after provider error: {}", provider_retry_count, e),
                        );
                        continue;
                    }
                    return Err(e);
                }
            };

            if let Some(tool_calls) = response.tool_calls.as_ref().filter(|tc| !tc.is_empty()) {
                // Thinking phase: notify the sender about each tool call.
                ctx.push_message(ChatMessage {
                    role: "assistant".to_string(),
                    content: ChatMessageContent::Text(response.content.clone().unwrap_or_default()),
                    tool_call_id: None,
                    tool_calls: Some(tool_calls.clone()),
                    name: None,
                });

                let mut pending_approval: Vec<PendingApproval> = Vec::new();
                let mut tool_limit_exceeded = false;

                for tc in tool_calls {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                    loop_count += 1;
                    if loop_count > self.config.max_tool_calls_per_turn {
                        warn!("max tool calls per turn exceeded");
                        tool_limit_exceeded = true;
                        break;
                    }

                    ctx.log_thinking("tool_call", tc.function.name.to_string());

                    let tool_ctx = ToolContext {
                        tool_call_id: tc.id.clone(),
                        user_id: ctx.user_id,
                        chat_id: Some(ctx.chat_id),
                        agent_id: Some(ctx.agent_id),
                        subagent_depth: ctx.subagent_depth,
                    };

                    let result = self
                        .tool_registry
                        .execute(
                            &tc.function.name,
                            args.clone(),
                            tool_ctx,
                            preapproved_tools,
                            force_tool_approval,
                        )
                        .await;

                    match result {
                        Ok(tool_result) => {
                            if let Some(audit) = &self.audit {
                                if tool_result.is_error {
                                    audit.failure(
                                        "tool_execution",
                                        tc.function.name.clone(),
                                        "execute",
                                        &tool_result.stderr,
                                    );
                                } else {
                                    audit.success(
                                        "tool_execution",
                                        tc.function.name.clone(),
                                        "execute",
                                    );
                                }
                            }
                            // Collect any queued Telegram media sends.
                            if !tool_result.is_error {
                                collect_media_from_result(ctx, &tool_result.metadata);
                            }
                            let content = if tool_result.is_error {
                                format!("Error: {}", tool_result.stderr)
                            } else {
                                tool_result.stdout.clone()
                            };
                            ctx.log_thinking(
                                "tool_result",
                                format!(
                                    "{}: {}",
                                    tc.function.name,
                                    truncate_to_chars(&content, 200)
                                ),
                            );
                            ctx.push_message(ChatMessage {
                                role: "tool".to_string(),
                                content: ChatMessageContent::Text(content),
                                tool_call_id: Some(tc.id.clone()),
                                tool_calls: None,
                                name: None,
                            });
                        }
                        Err(unly_core::Error::ToolDenied { reason }) => {
                            if let Some(audit) = &self.audit {
                                audit.denied(
                                    "tool_execution",
                                    tc.function.name.clone(),
                                    "execute",
                                    &reason,
                                );
                            }
                            pending_approval.push(PendingApproval {
                                tool_call_id: tc.id.clone(),
                                tool_name: tc.function.name.clone(),
                                args,
                                risk_level: "privileged".to_string(),
                            });
                        }
                        Err(e) => {
                            warn!(tool = %tc.function.name, error = %e, "tool error");
                            let err_msg = format!("Error: {}", e);
                            ctx.push_message(ChatMessage {
                                role: "tool".to_string(),
                                content: ChatMessageContent::Text(err_msg),
                                tool_call_id: Some(tc.id.clone()),
                                tool_calls: None,
                                name: None,
                            });
                        }
                    }
                }

                if !pending_approval.is_empty() {
                    ctx.pending_approvals = pending_approval.clone();
                    let _ = sender
                        .send(StreamEvent::ApprovalRequired {
                            pending: pending_approval,
                            ctx: Box::new(ctx.clone()),
                        })
                        .await;
                    return Ok(());
                }
                if tool_limit_exceeded {
                    return Err(unly_core::Error::Agent(
                        "max tool calls per turn exceeded".to_string(),
                    ));
                }

                continue;
            }

            let probe_text = response.content.clone().unwrap_or_default();
            if !forced_tool_retry
                && !tool_defs.is_empty()
                && looks_like_manual_confirmation_request(&probe_text)
            {
                forced_tool_retry = true;
                ctx.push_message(ChatMessage {
                    role: "assistant".to_string(),
                    content: ChatMessageContent::Text(probe_text),
                    tool_call_id: None,
                    tool_calls: None,
                    name: None,
                });
                ctx.push_message(ChatMessage {
                    role: "system".to_string(),
                    content: ChatMessageContent::Text(
                        "Do not ask for permission in plain text. If a tool is required, call the tool now and let runtime handle approval via Approve/Deny."
                            .to_string(),
                    ),
                    tool_call_id: None,
                    tool_calls: None,
                    name: None,
                });
                continue;
            }

            // --- RESPONSE PHASE: stream the final answer ---
            let stream_request = ChatRequest {
                model: ctx.model.clone(),
                messages: self.build_request_messages(ctx, memory_ctx.as_deref()),
                tools: None, // No tools on the final response stream
                temperature: Some(0.7),
                max_tokens: Some(4096),
                stream: true,
            };

            let _ = sender.send(StreamEvent::ResponseStart).await;

            let mut token_stream = provider.chat_stream(stream_request).await?;
            let mut full_content = String::new();

            while let Some(chunk) = token_stream.next().await {
                match chunk? {
                    StreamChunk::Delta(delta) => {
                        let _ = sender.send(StreamEvent::Token(delta.clone())).await;
                        full_content.push_str(&delta);
                    }
                    StreamChunk::Done(_) => {
                        break;
                    }
                }
            }

            // Strip thinking tags before saving and sending the final message.
            let mut final_text = strip_thinking_tags(&full_content);
            if self.config.force_plain_output {
                final_text = strip_html_tags(&final_text);
            }

            ctx.push_message(ChatMessage {
                role: "assistant".to_string(),
                content: ChatMessageContent::Text(full_content),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            });

            // Emit any queued media sends before the final text response.
            for media in ctx.pending_media.drain(..) {
                let _ = sender
                    .send(StreamEvent::SendMedia {
                        kind: media.kind,
                        path: media.path,
                        caption: media.caption,
                    })
                    .await;
            }

            let _ = sender.send(StreamEvent::Done(final_text.clone())).await;
            self.store_memory_turn(ctx, &user_msg_for_memory, &final_text)
                .await;
            return Ok(());
        }
    }

    /// Re-run pending tool calls after user approval.
    pub async fn process_approved(&self, ctx: &mut AgentContext) -> Result<AgentResponse> {
        ctx.system_prompt = self.build_system_prompt_with_hot_reload();
        let pending = std::mem::take(&mut ctx.pending_approvals);

        for approval in &pending {
            let tool_ctx = ToolContext {
                tool_call_id: approval.tool_call_id.clone(),
                user_id: ctx.user_id,
                chat_id: Some(ctx.chat_id),
                agent_id: Some(ctx.agent_id),
                subagent_depth: ctx.subagent_depth,
            };

            let result = self
                .tool_registry
                .execute(
                    &approval.tool_name,
                    approval.args.clone(),
                    tool_ctx,
                    true,
                    false,
                )
                .await;

            let content = match result {
                Ok(tool_result) => {
                    if tool_result.is_error {
                        format!("Error: {}", tool_result.stderr)
                    } else {
                        tool_result.stdout
                    }
                }
                Err(e) => format!("Error: {}", e),
            };

            ctx.push_message(ChatMessage {
                role: "tool".to_string(),
                content: ChatMessageContent::Text(content),
                tool_call_id: Some(approval.tool_call_id.clone()),
                tool_calls: None,
                name: None,
            });
        }

        let mut response = self.continue_from_tools(ctx).await?;
        if let AgentResponse::ApprovalRequired { .. } = response {
            for _ in 0..2 {
                response = self.continue_from_tools(ctx).await?;
                if !matches!(response, AgentResponse::ApprovalRequired { .. }) {
                    break;
                }
            }
        }
        if let AgentResponse::Text(ref text) = response {
            let user_msg = ctx
                .messages
                .iter()
                .rev()
                .find_map(|m| match (&m.role[..], &m.content) {
                    ("user", ChatMessageContent::Text(t)) => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            self.store_memory_turn(ctx, &user_msg, text).await;
        }
        Ok(response)
    }

    async fn continue_from_tools(&self, ctx: &mut AgentContext) -> Result<AgentResponse> {
        let provider = self.get_provider(&ctx.provider)?;
        let tool_defs = self.build_tool_defs(ctx);

        let request = ChatRequest {
            model: ctx.model.clone(),
            messages: self.build_request_messages(ctx, None),
            tools: if tool_defs.is_empty() {
                None
            } else {
                Some(tool_defs)
            },
            temperature: Some(0.7),
            max_tokens: Some(4096),
            stream: false,
        };

        let response = provider.chat(request).await?;
        let raw_text = response.content.unwrap_or_default();
        let mut text = strip_thinking_tags(&raw_text);
        if self.config.force_plain_output {
            text = strip_html_tags(&text);
        }

        ctx.push_message(ChatMessage {
            role: "assistant".to_string(),
            content: ChatMessageContent::Text(raw_text),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        });

        Ok(AgentResponse::Text(text))
    }

    fn build_tool_defs(&self, ctx: &AgentContext) -> Vec<unly_core::model::ToolDefinition> {
        let allow_spawn_subagent = should_expose_spawn_subagent(ctx);
        self.tool_registry
            .list_schemas()
            .into_iter()
            .filter(|schema| allow_spawn_subagent || schema.name != "spawn_subagent")
            .map(|schema| unly_core::model::ToolDefinition {
                r#type: "function".to_string(),
                function: unly_core::model::FunctionDefinition {
                    name: schema.name,
                    description: schema.description,
                    parameters: schema.parameters,
                },
            })
            .collect()
    }

    fn get_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        self.provider_registry
            .get(name)
            .or_else(|| self.provider_registry.default_provider().ok())
            .ok_or_else(|| unly_core::Error::ProviderNotFound(name.to_string()))
    }

    async fn build_memory_context(&self, ctx: &AgentContext, user_msg: &str) -> Option<String> {
        if !self.config.inject_memory_context {
            return None;
        }
        let mut contexts = Vec::new();
        let mut file_ctx: Option<String> = None;

        if self.config.use_file_memory_primary {
            file_ctx = self.build_file_memory_context();
            if let Some(primary) = file_ctx.as_ref() {
                contexts.push(primary.clone());
            }
        }

        if self.config.enable_db_memory_augmentation
            && let Some(db_ctx) = self
                .build_db_memory_context(ctx, user_msg, file_ctx.as_deref())
                .await
        {
            contexts.push(db_ctx);
        }

        if contexts.is_empty() {
            None
        } else {
            Some(contexts.join("\n\n---\n\n"))
        }
    }

    fn build_file_memory_context(&self) -> Option<String> {
        let index = std::fs::read_to_string(&self.config.file_memory_index_path).ok()?;
        let mut remaining = self.config.file_memory_max_total_chars.max(200);
        let mut lines = Vec::new();
        lines.push("# Global Memory Root".to_string());

        let trimmed_index =
            truncate_to_chars(&index, self.config.file_memory_max_chars_per_file.max(200));
        remaining = remaining.saturating_sub(trimmed_index.len());
        lines.push(format!(
            "## MEMORY.md ({})",
            self.config.file_memory_index_path
        ));
        lines.push(trimmed_index);

        let child_paths = parse_markdown_links(&index)
            .into_iter()
            .filter(|p| !p.starts_with("http://") && !p.starts_with("https://"));

        for rel in child_paths {
            if remaining < 64 {
                break;
            }
            let base = std::path::Path::new(&self.config.file_memory_index_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            let candidate = base.join(rel);
            if !candidate.exists() || !candidate.is_file() {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&candidate) else {
                continue;
            };
            let item_budget = self
                .config
                .file_memory_max_chars_per_file
                .min(remaining)
                .max(64);
            let trimmed = truncate_to_chars(&content, item_budget);
            remaining = remaining.saturating_sub(trimmed.len());
            lines.push(format!(
                "## Additional Memory Shard ({})",
                candidate.display()
            ));
            lines.push(trimmed);
        }

        Some(lines.join("\n"))
    }

    async fn build_db_memory_context(
        &self,
        ctx: &AgentContext,
        user_msg: &str,
        file_ctx: Option<&str>,
    ) -> Option<String> {
        let store = self.memory_store.as_ref()?;
        let top_k = self.config.memory_context_top_k;
        let threshold = self.config.memory_context_similarity_threshold;
        let semantic_query = build_semantic_query(user_msg, file_ctx);

        let mut scored = Vec::new();
        let chat_scope = MemoryScope::Chat(ctx.chat_id.to_string());
        if let Ok(results) = store
            .retrieve(MemoryQuery {
                scope: chat_scope,
                query: semantic_query.clone(),
                top_k,
                similarity_threshold: threshold,
            })
            .await
        {
            scored.extend(results);
        }

        if let Some(user_id) = ctx.user_id {
            let user_scope = MemoryScope::User(user_id.to_string());
            if let Ok(results) = store
                .retrieve(MemoryQuery {
                    scope: user_scope,
                    query: semantic_query.clone(),
                    top_k,
                    similarity_threshold: threshold,
                })
                .await
            {
                scored.extend(results);
            }
        }
        let agent_scope = MemoryScope::Agent(ctx.agent_id.to_string());
        if let Ok(results) = store
            .retrieve(MemoryQuery {
                scope: agent_scope,
                query: semantic_query,
                top_k,
                similarity_threshold: threshold,
            })
            .await
        {
            scored.extend(results);
        }

        if scored.is_empty() {
            return None;
        }

        scored.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut seen = std::collections::HashSet::new();
        let mut lines = Vec::new();
        lines.push("# Semantic Memory (DB Helper)".to_string());
        let mut total_chars = 0usize;
        for r in scored.into_iter().take(top_k) {
            let text = r.entry.content.replace('\n', " ");
            if !seen.insert(text.clone()) {
                continue;
            }
            let max_item = self
                .config
                .memory_context_max_chars_per_item
                .min(self.config.file_memory_max_chars_per_file.max(1))
                .max(1);
            let trimmed = truncate_to_chars(&text, max_item);
            total_chars += trimmed.len();
            if total_chars > self.config.memory_context_max_total_chars {
                break;
            }
            lines.push(format!(
                "- ({:.2}) [{}] {}",
                r.similarity,
                r.entry.scope.scope_type(),
                trimmed
            ));
        }
        Some(lines.join("\n"))
    }

    async fn store_memory_turn(&self, ctx: &AgentContext, user_msg: &str, assistant_msg: &str) {
        let user_clean = sanitize_memory_text(user_msg);
        let assistant_clean = sanitize_memory_text(assistant_msg);
        if user_clean.is_empty() && assistant_clean.is_empty() {
            return;
        }

        if self.config.memory_store_conversation_turns
            && let Some(store) = self.memory_store.as_ref()
        {
            let base_content = format!("User: {}\nAssistant: {}", user_clean, assistant_clean);
            let max_len = self.config.memory_store_max_chars_per_turn.max(64);
            let content = truncate_to_chars(&base_content, max_len);

            let metadata = serde_json::json!({
                "kind": "conversation_turn",
                "agent_id": ctx.agent_id.to_string(),
                "chat_id": ctx.chat_id.to_string(),
                "turn": ctx.turn_count,
            });

            let _ = store
                .store(
                    MemoryScope::Chat(ctx.chat_id.to_string()),
                    content.clone(),
                    Some("conversation".to_string()),
                    Some(ctx.agent_id.to_string()),
                    metadata.clone(),
                    None,
                )
                .await;

            if let Some(user_id) = ctx.user_id {
                let _ = store
                    .store(
                        MemoryScope::User(user_id.to_string()),
                        content,
                        Some("conversation".to_string()),
                        Some(ctx.agent_id.to_string()),
                        metadata,
                        None,
                    )
                    .await;
            }
            debug!(chat_id = %ctx.chat_id, "stored memory for turn");
        }

        if self.config.use_file_memory_primary && self.config.append_turns_to_today_memory {
            self.append_today_file_memory(ctx, &user_clean, &assistant_clean);
        }
        if self.config.use_file_memory_primary {
            self.append_durable_facts_to_memory_index(ctx, &user_clean);
        }
    }

    fn append_today_file_memory(&self, ctx: &AgentContext, user: &str, assistant: &str) {
        let path = std::path::Path::new(&self.config.file_memory_today_path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M").to_string();
        let entry = format!(
            "\n## {}\n- chat: `{}`\n- user: {}\n- assistant: {}\n",
            ts, ctx.chat_id, user, assistant
        );
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(entry.as_bytes());
        }
    }

    fn append_durable_facts_to_memory_index(&self, ctx: &AgentContext, user: &str) {
        let facts = extract_durable_memory_facts(user);
        if facts.is_empty() {
            return;
        }
        let path = std::path::Path::new(&self.config.file_memory_index_path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let existing = std::fs::read_to_string(path).unwrap_or_default();
        let mut new_entries = Vec::new();
        let existing_lower = existing.to_lowercase();
        for fact in facts {
            let fact_lower = fact.to_lowercase();
            if !existing_lower.contains(&fact_lower) {
                new_entries.push(fact);
            }
        }
        if new_entries.is_empty() {
            return;
        }

        let mut block = String::new();
        if !existing.contains("## Durable Facts") {
            block.push_str("\n## Durable Facts\n");
        }
        for fact in new_entries {
            block.push_str(&format!(
                "- {} (chat: {}, turn: {})\n",
                fact, ctx.chat_id, ctx.turn_count
            ));
        }

        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(block.as_bytes());
        }
    }

    fn build_request_messages(
        &self,
        ctx: &AgentContext,
        memory_ctx: Option<&str>,
    ) -> Vec<ChatMessage> {
        let mut messages = ctx.build_messages_with_memory(memory_ctx);
        messages.insert(
            1,
            ChatMessage {
                role: "system".to_string(),
                content: ChatMessageContent::Text(
                    "Per-request mandatory check: \
1) Skill relevance check against available skills; if relevant, apply those instructions. \
2) Memory relevance check against provided memory context; use only relevant durable facts. \
3) When user gives explicit durable preferences/profile/constraints, retain concise non-secret facts."
                        .to_string(),
                ),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            },
        );
        messages
    }
}

fn user_input_for_memory(input: &ChatMessageContent) -> String {
    match input {
        ChatMessageContent::Text(text) => text.clone(),
        ChatMessageContent::Parts(parts) => {
            let mut lines = Vec::new();
            for part in parts {
                match part {
                    unly_core::model::ContentPart::Text { text } => lines.push(text.clone()),
                    unly_core::model::ContentPart::ImageUrl { .. } => {
                        lines.push("[image attached]".to_string())
                    }
                }
            }
            lines.join("\n")
        }
    }
}

fn looks_like_manual_confirmation_request(text: &str) -> bool {
    let t = text.to_lowercase();
    let has_confirm_phrase = t.contains("confirm")
        || t.contains("do you approve")
        || t.contains("requires explicit approval")
        || t.contains("permission")
        || t.contains("want me to proceed")
        || t.contains("shall i proceed")
        || t.contains("подтверд")
        || t.contains("разреш")
        || t.contains("можно");
    has_confirm_phrase
        || t.contains("моя текущая среда не поддерживает")
        || t.contains("current environment does not support")
        || t.contains("you can either")
        || t.contains("что предпочтительнее")
}

fn should_expose_spawn_subagent(ctx: &AgentContext) -> bool {
    if ctx.subagent_depth > 0 {
        return false;
    }

    let Some(last_user_text) =
        ctx.messages
            .iter()
            .rev()
            .find_map(|m| match (&m.role[..], &m.content) {
                ("user", ChatMessageContent::Text(t)) => Some(t.as_str()),
                _ => None,
            })
    else {
        return false;
    };

    let t = last_user_text.to_lowercase();
    t.contains("/spawn_subagent")
        || t.contains("subagent")
        || t.contains("sub-agent")
        || t.contains("delegate")
        || t.contains("delegat")
        || t.contains("delegation")
        || t.contains("субагент")
        || t.contains("делег")
}

fn build_runtime_extensions_prompt(tool_registry: &ToolRegistry, config: &AppConfig) -> String {
    let skills = SkillLoader::load_from_dir(&config.plugins.skills_dir);
    let plugins = PluginLoader::load_from_dir(&config.plugins.plugins_dir);
    let active_skills: Vec<_> = skills.into_iter().filter(|s| s.enabled).collect();
    let active_plugins: Vec<_> = plugins.into_iter().filter(|p| p.enabled).collect();

    let policy = tool_registry.policy();
    let tool_lines = tool_registry
        .list_schemas()
        .into_iter()
        .map(|s| format!("- {} ({:?}): {}", s.name, s.risk, s.description))
        .collect::<Vec<_>>()
        .join("\n");

    let mut out = String::from(
        "\n\n---\n\n# Runtime Extensions (Hot Reloaded)\n\
Only skills/plugins listed in this section are currently active. \
If earlier sections conflict with this one, treat this section as authoritative.\n\n",
    );

    out.push_str("## Active Skills\n");
    if active_skills.is_empty() {
        out.push_str("- none\n\n");
    } else {
        out.push_str("### Skill Index\n\n");
        for skill in &active_skills {
            let id = skill
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(skill.meta.name.as_str());
            let hint = skill
                .instructions
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
                .map(|l| l.chars().take(140).collect::<String>())
                .unwrap_or_default();
            out.push_str(&format!(
                "- `{}` — {}{}\n",
                id,
                if skill.meta.description.is_empty() {
                    "(no description)"
                } else {
                    skill.meta.description.as_str()
                },
                if hint.is_empty() {
                    String::new()
                } else {
                    format!(" | hint: {}", hint)
                }
            ));
        }
        out.push_str("\n### Skill Details\n\n");
        for skill in &active_skills {
            out.push_str(&format!(
                "### {} — {}\n\n{}\n\n",
                skill.meta.name,
                if skill.meta.description.is_empty() {
                    "(no description)"
                } else {
                    &skill.meta.description
                },
                skill.instructions.trim()
            ));
        }
    }

    out.push_str("## Active Plugins\n");
    if active_plugins.is_empty() {
        out.push_str("- none\n\n");
    } else {
        for plugin in &active_plugins {
            out.push_str(&format!(
                "### {} — {}\n\n{}\n\n",
                plugin.meta.name,
                if plugin.meta.description.is_empty() {
                    "(no description)"
                } else {
                    &plugin.meta.description
                },
                plugin.instructions.trim()
            ));
        }
    }

    out.push_str("## Runtime Tools Snapshot\n");
    out.push_str(&tool_lines);
    out.push_str("\n\n## Runtime Policy Snapshot\n");
    out.push_str(&format!(
        "- require approval for privileged: {}\n- require approval for dangerous: {}\n- max tool execution seconds: {}\n- max concurrent tools: {}\n",
        policy.require_approval_for_privileged,
        policy.require_approval_for_dangerous,
        policy.max_execution_seconds,
        policy.max_concurrent,
    ));
    out.push_str(
        "- delegation policy: use `spawn_subagent` only when the user explicitly asks for delegation/subagents.\n\
- delegation exclusions: do not use subagents for simple single-step tasks.\n\
- recursion policy: subagents must not spawn child subagents.\n",
    );

    out
}

/// Response from the agent runtime (non-streaming mode).
#[derive(Debug, Clone)]
pub enum AgentResponse {
    /// Final text response.
    Text(String),
    /// One or more tool calls need user approval before proceeding.
    ApprovalRequired {
        pending: Vec<crate::context::PendingApproval>,
    },
}

impl AgentResponse {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            AgentResponse::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn is_approval_required(&self) -> bool {
        matches!(self, AgentResponse::ApprovalRequired { .. })
    }
}

/// Events emitted during a streaming agent run.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// The response phase has started (next events will be Tokens).
    ResponseStart,
    /// A partial response token from the LLM.
    Token(String),
    /// The final, complete user-visible response (thinking tags stripped).
    Done(String),
    /// One or more tool calls need user approval.
    ///
    /// Carries a snapshot of the `AgentContext` at the moment the approval
    /// requirement was detected so the handler can resume from the correct
    /// conversation state regardless of any session-store timing races.
    ApprovalRequired {
        pending: Vec<crate::context::PendingApproval>,
        ctx: Box<crate::context::AgentContext>,
    },
    /// A media file should be sent to the Telegram chat before the text response.
    SendMedia {
        kind: MediaKind,
        path: String,
        caption: Option<String>,
    },
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// If a tool result carries a `__telegram_send` metadata key, parse it and
/// push the media request onto `ctx.pending_media`.
fn collect_media_from_result(ctx: &mut AgentContext, meta: &serde_json::Value) {
    if let Some(send_info) = meta.get("__telegram_send") {
        let kind_str = send_info
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("document");
        let kind = match kind_str {
            "photo" => MediaKind::Photo,
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            "voice" => MediaKind::Voice,
            "animation" => MediaKind::Animation,
            _ => MediaKind::Document,
        };
        let path = match send_info.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return, // malformed — ignore
        };
        let caption = send_info
            .get("caption")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        ctx.pending_media.push(MediaSend {
            kind,
            path,
            caption,
        });
    }
}

/// Strip `<think>…</think>` blocks from the model output.
///
/// These blocks represent the model's internal reasoning (Mode 1) and must
/// never be surfaced directly to the user.
fn strip_thinking_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;

    loop {
        match remaining.find("<think>") {
            None => {
                result.push_str(remaining);
                break;
            }
            Some(start) => {
                result.push_str(&remaining[..start]);
                match remaining[start..].find("</think>") {
                    None => break, // malformed — drop the rest
                    Some(end_rel) => {
                        remaining = &remaining[start + end_rel + "</think>".len()..];
                    }
                }
            }
        }
    }

    // Clean up any extra leading/trailing whitespace left by removal.
    result.trim().to_string()
}

fn sanitize_memory_text(text: &str) -> String {
    let lowered = text.to_lowercase();
    let blocked = ["api_key", "token", "password", "secret", "bearer "];
    if blocked.iter().any(|p| lowered.contains(p)) {
        return "[redacted-sensitive-content]".to_string();
    }
    text.trim().to_string()
}

fn extract_durable_memory_facts(user_text: &str) -> Vec<String> {
    let text = user_text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let markers = [
        "запомни",
        "важно помнить",
        "предпочита",
        "называй меня",
        "меня зовут",
        "мой часовой пояс",
        "remember",
        "important to remember",
        "i prefer",
        "call me",
        "my name is",
        "my timezone",
        "always ",
        "never ",
    ];
    let lower = text.to_lowercase();
    if !markers.iter().any(|m| lower.contains(m)) {
        return Vec::new();
    }

    let mut out = Vec::new();
    for line in text.lines() {
        let candidate = line.trim().trim_start_matches("- ").trim();
        if candidate.len() < 8 {
            continue;
        }
        let c_lower = candidate.to_lowercase();
        if markers.iter().any(|m| c_lower.contains(m)) {
            out.push(truncate_to_chars(candidate, 220));
        }
    }
    if out.is_empty() {
        out.push(truncate_to_chars(text, 220));
    }
    out
}

fn truncate_to_chars(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn parse_markdown_links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut s = line;
        while let Some(start) = s.find('(') {
            let rest = &s[start + 1..];
            let Some(end) = rest.find(')') else {
                break;
            };
            let candidate = rest[..end].trim();
            if !candidate.is_empty() {
                out.push(candidate.to_string());
            }
            s = &rest[end + 1..];
        }
    }
    out
}

fn parse_prompt_token_limit(err: &unly_core::Error) -> Option<usize> {
    let unly_core::Error::Provider { message, .. } = err else {
        return None;
    };
    let marker = "limit of ";
    let start = message.find(marker)? + marker.len();
    let digits: String = message[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<usize>().ok()
    }
}

fn compress_context_for_token_limit(
    ctx: &mut AgentContext,
    memory_ctx: &mut Option<String>,
    token_limit: usize,
    round: u32,
) -> bool {
    let mut changed = false;
    if memory_ctx.is_some() {
        *memory_ctx = None;
        changed = true;
    }

    let per_message_cap = match round {
        0 => (token_limit / 16).clamp(800, 6000),
        1 => (token_limit / 24).clamp(500, 3000),
        _ => (token_limit / 32).clamp(300, 1600),
    };

    for msg in &mut ctx.messages {
        if compact_message_content(&mut msg.content, per_message_cap) {
            changed = true;
        }
    }

    let keep = match round {
        0 => 24,
        1 => 16,
        _ => 10,
    };
    if ctx.messages.len() > keep {
        let remove = ctx.messages.len() - keep;
        ctx.messages.drain(0..remove);
        changed = true;
    }

    changed
}

fn compact_message_content(content: &mut ChatMessageContent, max_chars: usize) -> bool {
    match content {
        ChatMessageContent::Text(text) => {
            if text.len() <= max_chars {
                return false;
            }
            *text = format!(
                "{}\n[truncated by runtime]",
                truncate_to_chars(text, max_chars)
            );
            true
        }
        ChatMessageContent::Parts(parts) => {
            let mut changed = false;
            for part in parts {
                if let ContentPart::Text { text } = part
                    && text.len() > max_chars
                {
                    *text = format!(
                        "{}\n[truncated by runtime]",
                        truncate_to_chars(text, max_chars)
                    );
                    changed = true;
                }
            }
            changed
        }
    }
}

fn build_semantic_query(user_msg: &str, file_ctx: Option<&str>) -> String {
    if let Some(ctx) = file_ctx {
        let compact = truncate_to_chars(&ctx.replace('\n', " "), 360);
        return format!("{} | global-memory: {}", user_msg, compact);
    }
    user_msg.to_string()
}

fn strip_html_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use unly_core::{ids::ChatId, permissions::PermissionSet};

    #[test]
    fn parse_prompt_token_limit_extracts_numeric_limit() {
        let err = unly_core::Error::provider(
            "copilot",
            r#"HTTP 400 — {"message":"prompt token count of 107239 exceeds the limit of 64000","code":"model_max_prompt_tokens_exceeded"}"#,
        );
        assert_eq!(parse_prompt_token_limit(&err), Some(64_000));
    }

    #[test]
    fn truncate_to_chars_handles_multibyte_boundaries() {
        let text = "─".repeat(80);
        let truncated = truncate_to_chars(&text, 200);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn compression_drops_memory_and_shrinks_history() {
        let mut ctx = AgentContext::new(
            ChatId::new(),
            None,
            PermissionSet::basic_user(),
            "copilot",
            "gpt-5",
            "system",
        );
        for i in 0..30 {
            ctx.push_message(ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: ChatMessageContent::Text("─".repeat(7_000)),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            });
        }

        let mut memory_ctx = Some("large memory context".repeat(500));
        let changed = compress_context_for_token_limit(&mut ctx, &mut memory_ctx, 64_000, 0);

        assert!(changed);
        assert!(memory_ctx.is_none());
        assert!(ctx.messages.len() <= 24);
        assert!(ctx.messages.iter().any(|m| matches!(
            &m.content,
            ChatMessageContent::Text(t) if t.contains("[truncated by runtime]")
        )));
    }
}
