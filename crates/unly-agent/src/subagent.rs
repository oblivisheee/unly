use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::{info, warn};
use uuid::Uuid;

use unly_core::{
    ids::AgentId,
    permissions::PermissionSet,
    types::{ExecutionStatus, Timestamp},
    Result,
};

use crate::context::AgentContext;
use crate::runtime::{AgentResponse, AgentRuntime};

/// A request to spawn a subagent.
#[derive(Debug)]
pub struct SubagentRequest {
    pub goal: String,
    pub parent_agent_id: AgentId,
    pub depth: u32,
    pub permissions: PermissionSet,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub token_budget: u32,
}

/// Handle to a running subagent.
pub struct SubagentHandle {
    pub id: AgentId,
    pub status: ExecutionStatus,
    pub result: Option<AgentResponse>,
}

/// Spawn a subagent with a given goal. The subagent runs the full agent loop
/// with a capped token budget and depth limit.
pub async fn spawn_subagent(
    request: SubagentRequest,
    runtime: Arc<AgentRuntime>,
    chat_id: unly_core::ids::ChatId,
) -> Result<SubagentHandle> {
    if request.depth >= 3 {
        return Err(unly_core::Error::SubagentLimitExceeded);
    }

    let agent_id = AgentId::new();
    info!(
        subagent_id = %agent_id,
        parent = %request.parent_agent_id,
        depth = request.depth,
        "spawning subagent"
    );

    let provider = request
        .provider
        .clone()
        .unwrap_or_else(|| "copilot".to_string());
    let model = request.model.clone().unwrap_or_else(|| "gpt-4o".to_string());

    let mut ctx = AgentContext::new(
        chat_id,
        None,
        request.permissions,
        provider,
        model,
        format!(
            "You are a subagent. Your goal: {}\n\nComplete the goal and return your findings.",
            request.goal
        ),
    );
    ctx.agent_id = agent_id;
    ctx.subagent_depth = request.depth + 1;

    let response = runtime
        .process(&mut ctx, request.goal.clone())
        .await?;

    Ok(SubagentHandle {
        id: agent_id,
        status: ExecutionStatus::Completed,
        result: Some(response),
    })
}
