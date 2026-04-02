use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use unly_core::{
    model::{ChatMessage, ChatMessageContent, ChatRequest, StreamChunk},
    provider::Provider,
    tool::ToolContext,
    Result,
};
use unly_memory::{MemoryQuery, MemoryScope, MemoryStore};
use unly_tools::ToolRegistry;

use crate::context::{AgentContext, PendingApproval};

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
        let user_msg = user_message.into();
        let memory_ctx = self.build_memory_context(ctx, &user_msg).await;
        let user_msg_for_memory = user_msg.clone();

        ctx.push_message(ChatMessage {
            role: "user".to_string(),
            content: ChatMessageContent::Text(user_msg.clone()),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        });

        ctx.turn_count += 1;
        if ctx.turn_count > self.config.max_turns {
            return Err(unly_core::Error::Agent("max turns exceeded".to_string()));
        }

        let tool_defs = self.build_tool_defs();
        let provider = self.get_provider(&ctx.provider)?;
        let mut loop_count = 0u32;

        let final_response = loop {
            ctx.trim_to(self.config.context_window_size);

            // --- THINKING PHASE: try to make a standard (non-streaming) call ---
            // Tool-call rounds are "thinking" — we use the non-streaming chat API.
            let request = ChatRequest {
                model: ctx.model.clone(),
                messages: ctx.build_messages_with_memory(memory_ctx.as_deref()),
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

            let response = provider.chat(request).await?;

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

                for tc in tool_calls {
                    let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(serde_json::Value::Object(Default::default()));

                    let tool_ctx = ToolContext {
                        tool_call_id: tc.id.clone(),
                        user_id: ctx.user_id,
                        chat_id: Some(ctx.chat_id),
                        agent_id: Some(ctx.agent_id),
                    };

                    loop_count += 1;
                    if loop_count > self.config.max_tool_calls_per_turn {
                        warn!("max tool calls per turn exceeded");
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
                        .execute(&tc.function.name, args.clone(), tool_ctx, false)
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
                                    &content[..content.len().min(200)]
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

                continue;
            }

            // --- RESPONSE PHASE: no tool calls — this is the final answer ---
            let raw_text = response.content.unwrap_or_default();
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
        let user_msg = user_message.into();
        let memory_ctx = self.build_memory_context(ctx, &user_msg).await;
        let user_msg_for_memory = user_msg.clone();

        ctx.push_message(ChatMessage {
            role: "user".to_string(),
            content: ChatMessageContent::Text(user_msg.clone()),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        });

        ctx.turn_count += 1;
        if ctx.turn_count > self.config.max_turns {
            return Err(unly_core::Error::Agent("max turns exceeded".to_string()));
        }

        let tool_defs = self.build_tool_defs();
        let provider = self.get_provider(&ctx.provider)?;
        let mut loop_count = 0u32;

        loop {
            ctx.trim_to(self.config.context_window_size);

            // Check if there are potential tool calls by doing a non-streaming call.
            // We only stream the FINAL response (when there are no more tools to call).
            let probe_request = ChatRequest {
                model: ctx.model.clone(),
                messages: ctx.build_messages_with_memory(memory_ctx.as_deref()),
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

            let response = provider.chat(probe_request).await?;

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

                for tc in tool_calls {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();

                    loop_count += 1;
                    if loop_count > self.config.max_tool_calls_per_turn {
                        warn!("max tool calls per turn exceeded");
                        break;
                    }

                    ctx.log_thinking("tool_call", tc.function.name.to_string());

                    let tool_ctx = ToolContext {
                        tool_call_id: tc.id.clone(),
                        user_id: ctx.user_id,
                        chat_id: Some(ctx.chat_id),
                        agent_id: Some(ctx.agent_id),
                    };

                    let result = self
                        .tool_registry
                        .execute(&tc.function.name, args.clone(), tool_ctx, false)
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
                                    &content[..content.len().min(200)]
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
                        .send(StreamEvent::ApprovalRequired(pending_approval))
                        .await;
                    return Ok(());
                }

                continue;
            }

            // --- RESPONSE PHASE: stream the final answer ---
            let stream_request = ChatRequest {
                model: ctx.model.clone(),
                messages: ctx.build_messages_with_memory(memory_ctx.as_deref()),
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

            let _ = sender.send(StreamEvent::Done(final_text.clone())).await;
            self.store_memory_turn(ctx, &user_msg_for_memory, &final_text)
                .await;
            return Ok(());
        }
    }

    /// Re-run pending tool calls after user approval.
    pub async fn process_approved(&self, ctx: &mut AgentContext) -> Result<AgentResponse> {
        let pending = std::mem::take(&mut ctx.pending_approvals);

        for approval in &pending {
            let tool_ctx = ToolContext {
                tool_call_id: approval.tool_call_id.clone(),
                user_id: ctx.user_id,
                chat_id: Some(ctx.chat_id),
                agent_id: Some(ctx.agent_id),
            };

            let result = self
                .tool_registry
                .execute(&approval.tool_name, approval.args.clone(), tool_ctx, true)
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

        let response = self.continue_from_tools(ctx).await?;
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
        let tool_defs = self.build_tool_defs();

        let request = ChatRequest {
            model: ctx.model.clone(),
            messages: ctx.build_messages_with_memory(None),
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

    fn build_tool_defs(&self) -> Vec<unly_core::model::ToolDefinition> {
        self.tool_registry
            .list_schemas()
            .into_iter()
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

        if self.config.enable_db_memory_augmentation {
            if let Some(db_ctx) = self
                .build_db_memory_context(ctx, user_msg, file_ctx.as_deref())
                .await
            {
                contexts.push(db_ctx);
            }
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
            let trimmed = if text.len() > max_item {
                format!("{}…", &text[..max_item])
            } else {
                text
            };
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
        if !self.config.memory_store_conversation_turns {
            return;
        }
        let Some(store) = self.memory_store.as_ref() else {
            return;
        };
        let user_clean = sanitize_memory_text(user_msg);
        let assistant_clean = sanitize_memory_text(assistant_msg);
        if user_clean.is_empty() && assistant_clean.is_empty() {
            return;
        }

        let base_content = format!("User: {}\nAssistant: {}", user_clean, assistant_clean);
        let max_len = self.config.memory_store_max_chars_per_turn.max(64);
        let content = if base_content.len() > max_len {
            base_content[..max_len].to_string()
        } else {
            base_content
        };

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

        if self.config.use_file_memory_primary && self.config.append_turns_to_today_memory {
            self.append_today_file_memory(ctx, &user_clean, &assistant_clean);
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
    ApprovalRequired(Vec<crate::context::PendingApproval>),
}

// ── helpers ──────────────────────────────────────────────────────────────────

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
