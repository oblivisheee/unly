use std::sync::Arc;
use tracing::warn;

use unly_core::{
    model::{ChatMessage, ChatMessageContent, ChatRequest},
    provider::Provider,
    tool::ToolContext,
    Result,
};
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
}

/// The main agent runtime.
///
/// Handles the agentic loop: receive message → plan → call tools → respond.
pub struct AgentRuntime {
    config: AgentRuntimeConfig,
    provider_registry: Arc<unly_providers::ProviderRegistry>,
    tool_registry: Arc<ToolRegistry>,
    audit: Option<Arc<unly_audit::AuditLogger>>,
}

impl AgentRuntime {
    pub fn new(
        config: AgentRuntimeConfig,
        provider_registry: Arc<unly_providers::ProviderRegistry>,
        tool_registry: Arc<ToolRegistry>,
        audit: Option<Arc<unly_audit::AuditLogger>>,
    ) -> Self {
        Self {
            config,
            provider_registry,
            tool_registry,
            audit,
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

        // Get tool schemas as tool definitions.
        let tool_defs: Vec<unly_core::model::ToolDefinition> = self
            .tool_registry
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
            .collect();

        let provider = self.get_provider(&ctx.provider)?;

        // Agentic loop.
        let mut loop_count = 0u32;

        let final_response = loop {
            ctx.trim_to(self.config.context_window_size);

            let request = ChatRequest {
                model: ctx.model.clone(),
                messages: ctx.build_messages(),
                tools: if tool_defs.is_empty() {
                    None
                } else {
                    Some(tool_defs.clone())
                },
                temperature: Some(0.7),
                max_tokens: Some(4096),
                stream: false,
            };

            let response = provider.chat(request).await?;

            // If there are tool calls, handle them.
            if let Some(tool_calls) = response.tool_calls.as_ref().filter(|tc| !tc.is_empty()) {
                // Push the assistant's tool call message.
                ctx.push_message(ChatMessage {
                    role: "assistant".to_string(),
                    content: ChatMessageContent::Text(
                        response.content.clone().unwrap_or_default(),
                    ),
                    tool_call_id: None,
                    tool_calls: Some(tool_calls.clone()),
                    name: None,
                });

                // Execute each tool call.
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

                    // Try executing; handle approval-required case.
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
                                tool_result.stdout
                            };

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
                            ctx.push_message(ChatMessage {
                                role: "tool".to_string(),
                                content: ChatMessageContent::Text(format!("Error: {}", e)),
                                tool_call_id: Some(tc.id.clone()),
                                tool_calls: None,
                                name: None,
                            });
                        }
                    }
                }

                // If there are pending approvals, return early with an approval request.
                if !pending_approval.is_empty() {
                    ctx.pending_approvals = pending_approval.clone();
                    return Ok(AgentResponse::ApprovalRequired {
                        pending: pending_approval,
                    });
                }

                // Continue the loop.
                continue;
            }

            // No tool calls — this is the final text response.
            let text = response.content.unwrap_or_default();
            ctx.push_message(ChatMessage {
                role: "assistant".to_string(),
                content: ChatMessageContent::Text(text.clone()),
                tool_call_id: None,
                tool_calls: None,
                name: None,
            });
            break text;
        };

        Ok(AgentResponse::Text(final_response))
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

        // Continue with the loop.
        self.continue_from_tools(ctx).await
    }

    async fn continue_from_tools(&self, ctx: &mut AgentContext) -> Result<AgentResponse> {
        let provider = self.get_provider(&ctx.provider)?;
        let tool_defs: Vec<unly_core::model::ToolDefinition> = self
            .tool_registry
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
            .collect();

        let request = ChatRequest {
            model: ctx.model.clone(),
            messages: ctx.build_messages(),
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
        let text = response.content.unwrap_or_default();

        ctx.push_message(ChatMessage {
            role: "assistant".to_string(),
            content: ChatMessageContent::Text(text.clone()),
            tool_call_id: None,
            tool_calls: None,
            name: None,
        });

        Ok(AgentResponse::Text(text))
    }

    fn get_provider(&self, name: &str) -> Result<Arc<dyn Provider>> {
        self.provider_registry
            .get(name)
            .or_else(|| self.provider_registry.default_provider().ok())
            .ok_or_else(|| unly_core::Error::ProviderNotFound(name.to_string()))
    }
}

/// Response from the agent runtime.
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
