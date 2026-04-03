use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::workspace;

/// Top-level application configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub telegram: TelegramConfig,
    pub database: DatabaseConfig,
    pub memory: MemoryConfig,
    pub providers: ProvidersConfig,
    pub tools: ToolsConfig,
    pub agent: AgentConfig,
    pub scheduler: SchedulerConfig,
    pub plugins: PluginsConfig,
    pub logging: LoggingConfig,
    pub webhook: WebhookConfig,
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot token from @BotFather. Can also be set via TELEGRAM_BOT_TOKEN env var.
    pub bot_token: String,
    /// Comma-separated list of allowed Telegram user IDs (admins).
    pub admin_user_ids: Vec<i64>,
    /// Comma-separated list of allowed user IDs (0 = allow all).
    pub allowed_user_ids: Vec<i64>,
    /// Whether to allow any user or only allowlisted ones.
    pub open_access: bool,
    /// Maximum number of messages to keep per chat for context window.
    pub context_window_size: usize,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token: String::new(),
            admin_user_ids: Vec::new(),
            allowed_user_ids: Vec::new(),
            open_access: false,
            context_window_size: 20,
        }
    }
}

/// Database backend selector.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DbType {
    #[default]
    Sqlite,
    Postgres,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Database backend: "sqlite" (default) or "postgres".
    pub db_type: DbType,
    /// Path to the SQLite database file (only used when db_type = "sqlite").
    pub path: PathBuf,
    /// PostgreSQL connection URL (only used when db_type = "postgres").
    /// Example: "postgresql://user:pass@localhost:5432/unly"
    pub postgres_url: Option<String>,
    /// Maximum connection pool size.
    pub max_connections: u32,
    /// Journal mode (WAL recommended for SQLite production).
    pub journal_mode: String,
    /// Whether to run migrations on startup.
    pub auto_migrate: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            db_type: DbType::Sqlite,
            path: workspace::default_db_path(),
            postgres_url: None,
            max_connections: 5,
            journal_mode: "WAL".to_string(),
            auto_migrate: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Whether the memory subsystem is enabled.
    pub enabled: bool,
    /// Provider to use for generating embeddings.
    pub embedding_provider: String,
    /// Model to use for embeddings (e.g. "text-embedding-3-small").
    pub embedding_model: String,
    /// Maximum number of results to return per semantic search.
    pub top_k: usize,
    /// Minimum cosine similarity score to include a result.
    pub similarity_threshold: f32,
    /// Retention days for raw messages (0 = keep forever).
    pub raw_retention_days: u64,
    /// Retention days for derived memory entries (0 = keep forever).
    pub memory_retention_days: u64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            embedding_provider: "copilot".to_string(),
            embedding_model: "text-embedding-3-small".to_string(),
            top_k: 5,
            similarity_threshold: 0.7,
            raw_retention_days: 90,
            memory_retention_days: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Default provider name.
    pub default_provider: String,
    /// Default model name.
    pub default_model: String,
    /// GitHub Copilot provider configuration.
    pub copilot: CopilotConfig,
    /// OpenAI-compatible provider configurations.
    pub openai_compatible: Vec<OpenAiCompatConfig>,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            default_provider: "copilot".to_string(),
            default_model: "gpt-4o".to_string(),
            copilot: CopilotConfig::default(),
            openai_compatible: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotConfig {
    /// Whether the GitHub Copilot provider is enabled.
    pub enabled: bool,
    /// GitHub OAuth app client ID for device flow.
    pub github_client_id: String,
    /// Path to store the cached GitHub OAuth token.
    pub token_cache_path: PathBuf,
    /// Base URL for the GitHub API.
    pub github_api_url: String,
    /// Base URL for the Copilot API.
    pub copilot_api_url: String,
}

impl Default for CopilotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            github_client_id: "Iv1.b507a08c87ecfe98".to_string(), // GitHub Copilot CLI client ID
            token_cache_path: workspace::default_token_cache_path(),
            github_api_url: "https://api.github.com".to_string(),
            copilot_api_url: "https://api.githubcopilot.com".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiCompatConfig {
    /// Unique name for this provider instance.
    pub name: String,
    /// Base URL (e.g. "https://api.openai.com/v1").
    pub base_url: String,
    /// API key. Can also be set via environment variable.
    pub api_key: String,
    /// Explicitly listed models (used if the provider doesn't support /models).
    pub models: Vec<String>,
    /// Whether this provider is enabled.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// List of enabled tool names (empty = all built-in safe tools enabled).
    pub enabled_tools: Vec<String>,
    /// List of explicitly disabled tool names.
    pub disabled_tools: Vec<String>,
    /// Whether privileged tools require interactive approval.
    pub require_approval_for_privileged: bool,
    /// Whether dangerous tools require interactive approval.
    pub require_approval_for_dangerous: bool,
    /// Maximum execution time for a single tool call in seconds.
    pub max_execution_seconds: u64,
    /// Maximum number of concurrent tool executions.
    pub max_concurrent_executions: usize,
    /// Shell command allowlist patterns (empty = deny all shell execution).
    pub shell_allowlist: Vec<String>,
    /// Working directory for shell tool execution.
    pub shell_working_dir: Option<PathBuf>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            enabled_tools: vec![
                "http_get".to_string(),
                "http_post".to_string(),
                "fs_read".to_string(),
                "fs_list".to_string(),
                "fs_write".to_string(),
                "git_status".to_string(),
                "git_log".to_string(),
                "bash".to_string(),
                "spawn_subagent".to_string(),
                "cron_job".to_string(),
            ],
            disabled_tools: Vec::new(),
            require_approval_for_privileged: true,
            require_approval_for_dangerous: true,
            max_execution_seconds: 30,
            max_concurrent_executions: 4,
            shell_allowlist: vec![
                r"^ls(\s|$)".to_string(),
                r"^pwd(\s|$)".to_string(),
                r"^cat\s+".to_string(),
                r"^echo\s+".to_string(),
            ],
            shell_working_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Maximum depth for nested subagent spawning.
    pub max_subagent_depth: u32,
    /// Maximum number of concurrent subagents.
    pub max_concurrent_subagents: usize,
    /// Maximum number of child subagents a single subagent can create.
    pub max_child_subagents_per_parent: usize,
    /// Maximum tokens per subagent run.
    pub subagent_token_budget: u32,
    /// Maximum tool calls per agent turn.
    pub max_tool_calls_per_turn: u32,
    /// Maximum turns per conversation before truncation.
    pub max_turns: u32,
    /// Enable semantic memory retrieval before each model turn.
    pub inject_memory_context: bool,
    /// Number of memory entries to inject into prompt context.
    pub memory_context_top_k: usize,
    /// Similarity threshold used for memory retrieval.
    pub memory_context_similarity_threshold: f32,
    /// Maximum characters per recalled memory snippet.
    pub memory_context_max_chars_per_item: usize,
    /// Maximum total characters for injected memory block.
    pub memory_context_max_total_chars: usize,
    /// Enable writing conversation turns into memory.
    pub memory_store_conversation_turns: bool,
    /// Maximum stored chars per memory turn record.
    pub memory_store_max_chars_per_turn: usize,
    /// Use file memory (`MEMORY.md` + linked files) as primary context.
    pub use_file_memory_primary: bool,
    /// Canonical memory index path.
    pub file_memory_index_path: PathBuf,
    /// Rolling AI-managed memory shard path.
    pub file_memory_today_path: PathBuf,
    /// Maximum chars per loaded memory file.
    pub file_memory_max_chars_per_file: usize,
    /// Maximum total chars for all loaded file memory.
    pub file_memory_max_total_chars: usize,
    /// Whether DB semantic memory augments file memory.
    pub enable_db_memory_augmentation: bool,
    /// Whether completed turns are appended to rolling memory shard file.
    pub append_turns_to_today_memory: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_subagent_depth: 3,
            max_concurrent_subagents: 4,
            max_child_subagents_per_parent: 5,
            subagent_token_budget: 8192,
            max_tool_calls_per_turn: 10,
            max_turns: 100,
            inject_memory_context: true,
            memory_context_top_k: 6,
            memory_context_similarity_threshold: 0.68,
            memory_context_max_chars_per_item: 240,
            memory_context_max_total_chars: 2000,
            memory_store_conversation_turns: true,
            memory_store_max_chars_per_turn: 1200,
            use_file_memory_primary: true,
            file_memory_index_path: workspace::memory_index_path(),
            file_memory_today_path: workspace::memory_today_path(),
            file_memory_max_chars_per_file: 1200,
            file_memory_max_total_chars: 3200,
            enable_db_memory_augmentation: true,
            append_turns_to_today_memory: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Whether the scheduler is enabled.
    pub enabled: bool,
    /// Maximum number of concurrent scheduled job executions.
    pub max_concurrent_jobs: usize,
    /// Default retry count for failed jobs.
    pub default_retry_count: u32,
    /// Default retry delay in seconds.
    pub default_retry_delay_seconds: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrent_jobs: 4,
            default_retry_count: 3,
            default_retry_delay_seconds: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginsConfig {
    /// Directory to load plugins from.
    pub plugins_dir: PathBuf,
    /// Whether unknown plugins are allowed (false = only listed plugins).
    pub allow_unknown: bool,
    /// Explicitly enabled plugin IDs.
    pub enabled: Vec<String>,
    /// Explicitly disabled plugin IDs.
    pub disabled: Vec<String>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            plugins_dir: workspace::workspace_dir().join("plugins"),
            allow_unknown: false,
            enabled: Vec::new(),
            disabled: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error).
    pub level: String,
    /// Whether to use JSON-structured logging.
    pub json: bool,
    /// Optional log file path (in addition to stderr).
    pub file: Option<PathBuf>,
    /// Whether to include sensitive data in logs (default: false).
    pub include_sensitive: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            json: false,
            file: None,
            include_sensitive: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Whether the webhook server is enabled.
    pub enabled: bool,
    /// Bind address for the webhook HTTP server.
    pub bind_addr: String,
    /// Port for the webhook server.
    pub port: u16,
    /// Shared secret for webhook verification (HMAC-SHA256).
    pub secret: String,
    /// Maximum payload size in bytes.
    pub max_payload_bytes: usize,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: "127.0.0.1".to_string(),
            port: 8080,
            secret: String::new(),
            max_payload_bytes: 1_048_576, // 1MB
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Whether to redact secrets in logs.
    pub redact_secrets: bool,
    /// Rate limit: maximum messages per user per minute.
    pub rate_limit_per_minute: u32,
    /// Whether to validate Telegram webhook secrets.
    pub validate_telegram_secret: bool,
    /// Telegram webhook secret token (optional).
    pub telegram_secret_token: Option<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            redact_secrets: true,
            rate_limit_per_minute: 30,
            validate_telegram_secret: false,
            telegram_secret_token: None,
        }
    }
}
