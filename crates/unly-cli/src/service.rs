use anyhow::Result;
use std::sync::Arc;

use unly_agent::{AgentRuntime, AgentRuntimeConfig};
use unly_audit::AuditLogger;
use unly_config::{AppConfig, workspace};
use unly_db::Database;
use unly_memory::MemoryStore;
use unly_plugins::{PluginLoader, SkillLoader};
use unly_providers::{
    ProviderRegistry, copilot::CopilotProvider, openai_compat::OpenAiCompatProvider,
};
use unly_tools::{
    ToolRegistry,
    builtin::{
        CronJobTool, FsCopyTool, FsDeleteTool, FsGrepTool, FsListTool, FsMkdirTool, FsMoveTool,
        FsReadTool, FsStatTool, FsWriteTool, GitLogTool, GitStatusTool, HttpGetTool, HttpPostTool,
        PluginCreateTool, PluginDisableTool, PluginEnableTool, PluginListTool, PluginRemoveTool,
        SelfConfigGetTool, SelfConfigSetTool, SkillCreateTool, SkillDisableTool, SkillEnableTool,
        SkillInspectTool, SkillListTool, SkillRemoveTool, SkillSearchTool, SpawnSubagentTool,
        TelegramSendAnimationTool, TelegramSendAudioTool, TelegramSendDocumentTool,
        TelegramSendPhotoTool, TelegramSendVideoTool, TelegramSendVoiceTool, create_scheduler,
    },
    policy::ExecutionPolicy,
};

fn ensure_core_native_tools(mut enabled: Vec<String>) -> Vec<String> {
    if enabled.is_empty() {
        return enabled;
    }
    for name in [
        "fs_read",
        "fs_list",
        "fs_write",
        "fs_delete",
        "fs_copy",
        "fs_move",
        "fs_mkdir",
        "fs_stat",
        "fs_grep",
        "spawn_subagent",
        "cron_job",
        "skill_list",
        "skill_create",
        "skill_search",
        "skill_inspect",
        "skill_enable",
        "skill_disable",
        "skill_remove",
        "self_config_get",
        "self_config_set",
        "plugin_list",
        "plugin_create",
        "plugin_enable",
        "plugin_disable",
        "plugin_remove",
        "telegram_send_photo",
        "telegram_send_document",
        "telegram_send_video",
        "telegram_send_audio",
        "telegram_send_voice",
        "telegram_send_animation",
    ] {
        if !enabled.iter().any(|t| t == name) {
            enabled.push(name.to_string());
        }
    }
    enabled
}

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

/// Header injected before skill instructions in the system prompt.
const SKILLS_SECTION_HEADER: &str = "# Skills\n\nThe following skills are available and their instructions should be \
followed when relevant:\n\n";

/// Header injected before plugin instructions in the system prompt.
const PLUGINS_SECTION_HEADER: &str = "# Plugins\n\nThe following plugins are installed and their instructions should be \
followed when relevant:\n\n";

/// Register the skill/plugin self-configuration tools into `registry`.
fn register_management_tools(registry: &mut ToolRegistry, config: &AppConfig) {
    let skills_dir = config.plugins.skills_dir.clone();
    let plugins_dir = config.plugins.plugins_dir.clone();
    registry.register(SkillListTool {
        skills_dir: skills_dir.clone(),
    });
    registry.register(SkillCreateTool {
        skills_dir: skills_dir.clone(),
    });
    registry.register(SkillEnableTool {
        skills_dir: skills_dir.clone(),
    });
    registry.register(SkillSearchTool {
        skills_dir: skills_dir.clone(),
    });
    registry.register(SkillInspectTool {
        skills_dir: skills_dir.clone(),
    });
    registry.register(SkillDisableTool {
        skills_dir: skills_dir.clone(),
    });
    registry.register(SkillRemoveTool { skills_dir });
    registry.register(SelfConfigGetTool);
    registry.register(SelfConfigSetTool);
    registry.register(PluginListTool {
        plugins_dir: plugins_dir.clone(),
    });
    registry.register(PluginCreateTool {
        plugins_dir: plugins_dir.clone(),
    });
    registry.register(PluginEnableTool {
        plugins_dir: plugins_dir.clone(),
    });
    registry.register(PluginDisableTool {
        plugins_dir: plugins_dir.clone(),
    });
    registry.register(PluginRemoveTool { plugins_dir });
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
        ensure_core_native_tools(config.tools.enabled_tools.clone()),
        config.tools.disabled_tools.clone(),
    );

    registry.register(HttpGetTool::new());
    registry.register(HttpPostTool::new());
    registry.register(FsReadTool);
    registry.register(FsListTool);
    registry.register(FsWriteTool);
    registry.register(FsDeleteTool);
    registry.register(FsCopyTool);
    registry.register(FsMoveTool);
    registry.register(FsMkdirTool);
    registry.register(FsStatTool);
    registry.register(FsGrepTool);
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
    registry.register(TelegramSendPhotoTool);
    registry.register(TelegramSendDocumentTool);
    registry.register(TelegramSendVideoTool);
    registry.register(TelegramSendAudioTool);
    registry.register(TelegramSendVoiceTool);
    registry.register(TelegramSendAnimationTool);
    registry.register(SpawnSubagentTool);
    register_management_tools(&mut registry, config);

    Arc::new(registry)
}

pub fn build_tools_with_scheduler(
    config: &AppConfig,
    db: Database,
) -> (Arc<ToolRegistry>, Arc<unly_scheduler::Scheduler>) {
    let policy = ExecutionPolicy {
        require_approval_for_privileged: config.tools.require_approval_for_privileged,
        require_approval_for_dangerous: config.tools.require_approval_for_dangerous,
        max_execution_seconds: config.tools.max_execution_seconds,
        max_concurrent: config.tools.max_concurrent_executions,
        shell_allowlist: config.tools.shell_allowlist.clone(),
    };

    let mut registry = ToolRegistry::new(
        policy,
        ensure_core_native_tools(config.tools.enabled_tools.clone()),
        config.tools.disabled_tools.clone(),
    );

    registry.register(HttpGetTool::new());
    registry.register(HttpPostTool::new());
    registry.register(FsReadTool);
    registry.register(FsListTool);
    registry.register(FsWriteTool);
    registry.register(FsDeleteTool);
    registry.register(FsCopyTool);
    registry.register(FsMoveTool);
    registry.register(FsMkdirTool);
    registry.register(FsStatTool);
    registry.register(FsGrepTool);
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
    registry.register(TelegramSendPhotoTool);
    registry.register(TelegramSendDocumentTool);
    registry.register(TelegramSendVideoTool);
    registry.register(TelegramSendAudioTool);
    registry.register(TelegramSendVoiceTool);
    registry.register(TelegramSendAnimationTool);
    registry.register(SpawnSubagentTool);
    let scheduler = create_scheduler(db.clone(), &config.scheduler);
    registry.register(CronJobTool::new(db, scheduler.clone()));
    register_management_tools(&mut registry, config);

    (Arc::new(registry), scheduler)
}

/// Load the agent system prompt from IDENTITY.md + SOUL.md (+ BOOT.md on first start).
///
/// If these files don't exist in the workspace, bundled defaults are used.
/// This function also writes the default files if they are absent, so the user
/// can discover and customise them.
///
/// Active skills found in the configured skills directory are appended as an
/// additional `# Skills` section so the agent knows about them at runtime.
pub fn load_system_prompt(tool_registry: &ToolRegistry, config: &AppConfig) -> String {
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

    // Load enabled skills and build a skills section for the prompt.
    let skills_section = {
        let skills = SkillLoader::load_from_dir(&config.plugins.skills_dir);
        let active: Vec<_> = skills.into_iter().filter(|s| s.enabled).collect();
        if active.is_empty() {
            String::new()
        } else {
            let mut section = SKILLS_SECTION_HEADER.to_string();
            section.push_str("## Skill Index\n\n");
            for skill in &active {
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
                section.push_str(&format!(
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
            section.push_str("\n## Skill Details\n\n");
            for skill in &active {
                section.push_str(&format!(
                    "## {} — {}\n\n{}\n\n",
                    skill.meta.name,
                    skill.meta.description,
                    skill.instructions.trim()
                ));
            }
            section
        }
    };

    // Load enabled plugins and build a plugins section for the prompt.
    let plugins_section = {
        let plugins = PluginLoader::load_from_dir(&config.plugins.plugins_dir);
        let active: Vec<_> = plugins.into_iter().filter(|p| p.enabled).collect();
        if active.is_empty() {
            String::new()
        } else {
            let mut section = PLUGINS_SECTION_HEADER.to_string();
            for plugin in &active {
                section.push_str(&format!(
                    "## {} — {}\n\n{}\n\n",
                    plugin.meta.name,
                    plugin.meta.description,
                    plugin.instructions.trim()
                ));
            }
            section
        }
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
        (false, false) => "- Tool execution policy: all tools are pre-authorized by the operator. \
Execute tools directly whenever needed; do not ask the user for permission before running any tool."
            .to_string(),
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
    let approval_behavior_directive = "- Approval behavior: never ask for permission in plain text (no \"confirm?\", \"allow?\", \"shall I proceed?\"). \
If a tool is needed, call the tool immediately and let runtime handle Approve/Deny. \
For cron tasks, prefer the native `cron_job` tool. Use `spawn_subagent` only when the user explicitly asks for a subagent/delegation, and never for simple single-step tasks. \
When creating cron jobs, set `notify_mode` explicitly from user intent: use `silent` only when user explicitly asks for no notifications; otherwise use `message`."
        .to_string();

    let capabilities = format!(
        r#"
# Runtime Capabilities
- Tools currently available in this runtime:
{}
{}
{}
- Native runtime capabilities include:
  - `spawn_subagent` for background delegated tasks with full runtime permissions (after approval by policy).
  - `cron_job` for scheduled tasks (`create/list/enable/disable/run_now/delete`) with `notify_mode` support.
  - `telegram_send_*` media tools (`photo/document/video/audio/voice/animation`) for sending local files to the current Telegram chat.
  - terminal command execution tools (subject to policy/permissions).
- Policy details:
  - require approval for privileged: {}
  - require approval for dangerous: {}
  - max tool execution seconds: {}
  - max concurrent tools: {}
- You have persistent semantic memory and should retain durable non-secret user context.
- Use subagents only when the user explicitly requests delegation/subagent execution, and never for simple one-step actions.
- Think before speaking: keep planning/tool execution in the internal thinking phase; only return final user-facing output.
- Support both model types: with explicit reasoning channels and without them.
- Never fabricate outcomes, access, or tool results; explicitly state limitations when access is unavailable.
"#,
        tool_lines,
        approval_directive,
        approval_behavior_directive,
        policy.require_approval_for_privileged,
        policy.require_approval_for_dangerous,
        policy.max_execution_seconds,
        policy.max_concurrent
    );

    // Assemble prompt sections; optional sections are only added when non-empty.
    let mut sections: Vec<&str> = vec![
        identity.trim(),
        soul.trim(),
        tools_profile.trim(),
        memory_index.trim(),
    ];
    if boot_mode {
        sections.push(boot.trim());
    }
    if !skills_section.is_empty() {
        sections.push(skills_section.trim());
    }
    if !plugins_section.is_empty() {
        sections.push(plugins_section.trim());
    }
    sections.push(capabilities.trim());

    sections.join("\n\n---\n\n")
}

/// Build the agent runtime from config.
pub fn build_runtime(
    config: &AppConfig,
    provider_registry: Arc<ProviderRegistry>,
    tool_registry: Arc<ToolRegistry>,
    db: Database,
    audit: Option<Arc<AuditLogger>>,
) -> Arc<AgentRuntime> {
    let system_prompt = load_system_prompt(tool_registry.as_ref(), config);
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
            app_config: Some(config.clone()),
        },
        provider_registry,
        tool_registry,
        audit,
        memory_store,
    ))
}
