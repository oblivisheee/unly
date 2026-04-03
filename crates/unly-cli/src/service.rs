use anyhow::Result;
use std::sync::Arc;

use unly_agent::{AgentRuntime, AgentRuntimeConfig};
use unly_audit::AuditLogger;
use unly_config::{workspace, AppConfig};
use unly_db::Database;
use unly_memory::MemoryStore;
use unly_providers::{
    copilot::CopilotProvider, openai_compat::OpenAiCompatProvider, ProviderRegistry,
};
use unly_tools::{
    builtin::{FsListTool, FsReadTool, GitLogTool, GitStatusTool, HttpGetTool, HttpPostTool},
    policy::ExecutionPolicy,
    ToolRegistry,
};

/// Build the provider registry from config.
pub async fn build_providers(config: &AppConfig) -> Result<Arc<ProviderRegistry>> {
    let registry = Arc::new(ProviderRegistry::new(
        &config.providers.default_provider,
        &config.providers.default_model,
    ));

    // GitHub Copilot provider.
    if config.providers.copilot.enabled {
        let provider = CopilotProvider::new(
            config.providers.copilot.github_client_id.clone(),
            config.providers.copilot.token_cache_path.clone(),
            config.providers.copilot.copilot_api_url.clone(),
        );
        // Load cached token if available.
        provider.init_from_cache();
        registry.register(Arc::new(provider));
    }

    // OpenAI-compatible providers.
    for oc in &config.providers.openai_compatible {
        if !oc.enabled {
            continue;
        }
        let provider = OpenAiCompatProvider::new(
            oc.name.clone(),
            oc.base_url.clone(),
            oc.api_key.clone(),
            oc.models.clone(),
        );
        registry.register(Arc::new(provider));
    }

    Ok(registry)
}

/// Build the tool registry from config.
pub fn build_tools(config: &AppConfig) -> Arc<ToolRegistry> {
    let policy = ExecutionPolicy {
        require_approval_for_privileged: config.tools.require_approval_for_privileged,
        require_approval_for_dangerous: config.tools.require_approval_for_dangerous,
        max_execution_seconds: config.tools.max_execution_seconds,
        max_concurrent: config.tools.max_concurrent_executions,
        shell_allowlist: config.tools.shell_allowlist.clone(),
    };

    let mut registry = ToolRegistry::new(
        policy,
        config.tools.enabled_tools.clone(),
        config.tools.disabled_tools.clone(),
    );

    registry.register(HttpGetTool::new());
    registry.register(HttpPostTool::new());
    registry.register(FsReadTool);
    registry.register(FsListTool);
    registry.register(GitStatusTool);
    registry.register(GitLogTool);
    registry.register(unly_tools::builtin::ShellTool::new(
        config.tools.shell_allowlist.clone(),
        config.tools.shell_working_dir.clone(),
        config.tools.require_approval_for_dangerous,
    ));
    registry.register(unly_tools::builtin::BashTool::new(
        config.tools.shell_allowlist.clone(),
        config.tools.shell_working_dir.clone(),
        config.tools.require_approval_for_dangerous,
    ));

    Arc::new(registry)
}

/// Load the agent system prompt from IDENTITY.md + SOUL.md (+ BOOT.md on first start).
///
/// If these files don't exist in the workspace, bundled defaults are used.
/// This function also writes the default files if they are absent, so the user
/// can discover and customise them.
pub fn load_system_prompt(tool_registry: &ToolRegistry) -> String {
    let id_path = workspace::identity_path();
    let soul_path = workspace::soul_path();
    let boot_path = workspace::boot_path();
    let tools_path = workspace::tools_path();
    let memory_path = workspace::memory_index_path();

    // Write defaults if the files don't exist yet.
    if !id_path.exists() {
        if let Some(parent) = id_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&id_path, workspace::DEFAULT_IDENTITY);
    }
    let boot_mode = workspace::is_boot_mode();
    if boot_mode && !boot_path.exists() {
        if let Some(parent) = boot_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&boot_path, workspace::DEFAULT_BOOT);
    }
    if !soul_path.exists() {
        if let Some(parent) = soul_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&soul_path, workspace::DEFAULT_SOUL);
    }
    if !tools_path.exists() {
        if let Some(parent) = tools_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&tools_path, workspace::DEFAULT_TOOLS);
    }
    if !memory_path.exists() {
        if let Some(parent) = memory_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&memory_path, workspace::DEFAULT_MEMORY);
    }
    let memory_today = workspace::memory_today_path();
    if !memory_today.exists() {
        if let Some(parent) = memory_today.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&memory_today, workspace::DEFAULT_MEMORY_TODAY);
    }

    let identity = std::fs::read_to_string(&id_path)
        .unwrap_or_else(|_| workspace::DEFAULT_IDENTITY.to_string());
    let soul =
        std::fs::read_to_string(&soul_path).unwrap_or_else(|_| workspace::DEFAULT_SOUL.to_string());
    let tools_profile = std::fs::read_to_string(&tools_path)
        .unwrap_or_else(|_| workspace::DEFAULT_TOOLS.to_string());
    let memory_index = std::fs::read_to_string(&memory_path)
        .unwrap_or_else(|_| workspace::DEFAULT_MEMORY.to_string());
    let boot = if boot_mode {
        std::fs::read_to_string(&boot_path).unwrap_or_else(|_| workspace::DEFAULT_BOOT.to_string())
    } else {
        String::new()
    };

    let policy = tool_registry.policy();
    let tool_lines = tool_registry
        .list_schemas()
        .into_iter()
        .map(|s| format!("- {} ({:?}): {}", s.name, s.risk, s.description))
        .collect::<Vec<_>>()
        .join("\n");

    // Generate an unambiguous approval-policy directive so the agent never
    // second-guesses itself when the operator has already pre-authorized tools.
    let approval_directive = match (
        policy.require_approval_for_privileged,
        policy.require_approval_for_dangerous,
    ) {
        (false, false) => {
            "- Tool execution policy: all tools are pre-authorized by the operator. \
Execute tools directly whenever needed; do not ask the user for permission before running any tool."
                .to_string()
        }
        (false, true) => {
            "- Tool execution policy: privileged tools are pre-authorized (execute directly, \
no user confirmation needed). Dangerous tools still require explicit user approval."
                .to_string()
        }
        (true, false) => {
            "- Tool execution policy: dangerous tools are pre-authorized (execute directly, \
no user confirmation needed). Privileged tools still require explicit user approval."
                .to_string()
        }
        (true, true) => {
            "- Tool execution policy: both privileged and dangerous tools require explicit \
user approval before execution."
                .to_string()
        }
    };

    let capabilities = format!(
        r#"
# Runtime Capabilities
- Tools currently available in this runtime:
{}
{}
- Native runtime capabilities include subagent spawning and terminal command execution (subject to policy/permissions).
- Policy details:
  - require approval for privileged: {}
  - require approval for dangerous: {}
  - max tool execution seconds: {}
  - max concurrent tools: {}
- You have persistent semantic memory and should retain durable non-secret user context.
- You can use subagents for focused decomposition when it materially improves task quality.
- Think before speaking: keep planning/tool execution in the internal thinking phase; only return final user-facing output.
- Support both model types: with explicit reasoning channels and without them.
- Never fabricate outcomes, access, or tool results; explicitly state limitations when access is unavailable.
"#,
        tool_lines,
        approval_directive,
        policy.require_approval_for_privileged,
        policy.require_approval_for_dangerous,
        policy.max_execution_seconds,
        policy.max_concurrent
    );

    if boot_mode {
        format!(
            "{}\n\n---\n\n{}\n\n---\n\n{}\n\n---\n\n{}\n\n---\n\n{}\n\n---\n\n{}",
            identity.trim(),
            soul.trim(),
            tools_profile.trim(),
            memory_index.trim(),
            boot.trim(),
            capabilities.trim()
        )
    } else {
        format!(
            "{}\n\n---\n\n{}\n\n---\n\n{}\n\n---\n\n{}\n\n---\n\n{}",
            identity.trim(),
            soul.trim(),
            tools_profile.trim(),
            memory_index.trim(),
            capabilities.trim()
        )
    }
}

/// Build the agent runtime from config.
pub fn build_runtime(
    config: &AppConfig,
    provider_registry: Arc<ProviderRegistry>,
    tool_registry: Arc<ToolRegistry>,
    db: Database,
    audit: Option<Arc<AuditLogger>>,
) -> Arc<AgentRuntime> {
    let system_prompt = load_system_prompt(tool_registry.as_ref());
    let memory_store = if config.memory.enabled {
        provider_registry
            .get(&config.memory.embedding_provider)
            .map(|embedding_provider| {
                Arc::new(MemoryStore::new(
                    db.clone(),
                    embedding_provider,
                    config.memory.embedding_model.clone(),
                    config.memory.top_k,
                    config.memory.similarity_threshold,
                ))
            })
    } else {
        None
    };

    Arc::new(AgentRuntime::new(
        AgentRuntimeConfig {
            system_prompt,
            default_provider: config.providers.default_provider.clone(),
            default_model: config.providers.default_model.clone(),
            max_tool_calls_per_turn: config.agent.max_tool_calls_per_turn,
            max_turns: config.agent.max_turns,
            context_window_size: config.telegram.context_window_size,
            inject_memory_context: config.agent.inject_memory_context,
            memory_context_top_k: config.agent.memory_context_top_k,
            memory_context_similarity_threshold: config.agent.memory_context_similarity_threshold,
            memory_context_max_chars_per_item: config.agent.memory_context_max_chars_per_item,
            memory_context_max_total_chars: config.agent.memory_context_max_total_chars,
            memory_store_conversation_turns: config.agent.memory_store_conversation_turns,
            memory_store_max_chars_per_turn: config.agent.memory_store_max_chars_per_turn,
            use_file_memory_primary: config.agent.use_file_memory_primary,
            file_memory_index_path: config
                .agent
                .file_memory_index_path
                .to_string_lossy()
                .to_string(),
            file_memory_today_path: config
                .agent
                .file_memory_today_path
                .to_string_lossy()
                .to_string(),
            file_memory_max_chars_per_file: config.agent.file_memory_max_chars_per_file,
            file_memory_max_total_chars: config.agent.file_memory_max_total_chars,
            enable_db_memory_augmentation: config.agent.enable_db_memory_augmentation,
            append_turns_to_today_memory: config.agent.append_turns_to_today_memory,
            force_plain_output: false,
        },
        provider_registry,
        tool_registry,
        audit,
        memory_store,
    ))
}
