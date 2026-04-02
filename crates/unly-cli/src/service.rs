use anyhow::Result;
use std::sync::Arc;

use unly_agent::{AgentRuntime, AgentRuntimeConfig};
use unly_audit::AuditLogger;
use unly_config::{AppConfig, workspace};
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

    if !config.tools.shell_allowlist.is_empty() {
        registry.register(unly_tools::builtin::ShellTool::new(
            config.tools.shell_allowlist.clone(),
            config.tools.shell_working_dir.clone(),
        ));
    }

    Arc::new(registry)
}

/// Load the agent system prompt from IDENTITY.md + BOOT.md.
///
/// If these files don't exist in the workspace, the bundled defaults are used.
/// This function also writes the default files if they are absent, so the user
/// can discover and customise them.
pub fn load_system_prompt() -> String {
    let id_path = workspace::identity_path();
    let boot_path = workspace::boot_path();

    // Write defaults if the files don't exist yet.
    if !id_path.exists() {
        if let Some(parent) = id_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&id_path, workspace::DEFAULT_IDENTITY);
    }
    if !boot_path.exists() {
        if let Some(parent) = boot_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&boot_path, workspace::DEFAULT_BOOT);
    }

    let identity = std::fs::read_to_string(&id_path)
        .unwrap_or_else(|_| workspace::DEFAULT_IDENTITY.to_string());
    let boot = std::fs::read_to_string(&boot_path)
        .unwrap_or_else(|_| workspace::DEFAULT_BOOT.to_string());

    format!("{}\n\n---\n\n{}", identity.trim(), boot.trim())
}

/// Build the agent runtime from config.
pub fn build_runtime(
    config: &AppConfig,
    provider_registry: Arc<ProviderRegistry>,
    tool_registry: Arc<ToolRegistry>,
    audit: Option<Arc<AuditLogger>>,
) -> Arc<AgentRuntime> {
    let system_prompt = load_system_prompt();

    Arc::new(AgentRuntime::new(
        AgentRuntimeConfig {
            system_prompt,
            default_provider: config.providers.default_provider.clone(),
            default_model: config.providers.default_model.clone(),
            max_tool_calls_per_turn: config.agent.max_tool_calls_per_turn,
            max_turns: config.agent.max_turns,
            context_window_size: config.telegram.context_window_size,
        },
        provider_registry,
        tool_registry,
        audit,
    ))
}
