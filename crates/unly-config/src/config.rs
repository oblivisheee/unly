use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            telegram: TelegramConfig::default(),
            database: DatabaseConfig::default(),
            memory: MemoryConfig::default(),
            providers: ProvidersConfig::default(),
            tools: ToolsConfig::default(),
            agent: AgentConfig::default(),
            scheduler: SchedulerConfig::default(),
            plugins: PluginsConfig::default(),
            logging: LoggingConfig::default(),
            webhook: WebhookConfig::default(),
            security: SecurityConfig::default(),
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Path to the SQLite database file.
    pub path: PathBuf,
    /// Maximum connection pool size.
    pub max_connections: u32,
    /// Journal mode (WAL recommended for production).
    pub journal_mode: String,
    /// Whether to run migrations on startup.
    pub auto_migrate: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("data/unly.sqlite"),
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
            token_cache_path: PathBuf::from("data/github_token.json"),
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
                "git_status".to_string(),
                "git_log".to_string(),
            ],
            disabled_tools: Vec::new(),
            require_approval_for_privileged: true,
            require_approval_for_dangerous: true,
            max_execution_seconds: 30,
            max_concurrent_executions: 4,
            shell_allowlist: Vec::new(),
            shell_working_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// System prompt prefix injected into every conversation.
    pub system_prompt: String,
    /// Maximum depth for nested subagent spawning.
    pub max_subagent_depth: u32,
    /// Maximum number of concurrent subagents.
    pub max_concurrent_subagents: usize,
    /// Maximum tokens per subagent run.
    pub subagent_token_budget: u32,
    /// Maximum tool calls per agent turn.
    pub max_tool_calls_per_turn: u32,
    /// Maximum turns per conversation before truncation.
    pub max_turns: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: "You are Unly, a helpful personal AI agent. You have access to tools and can help with a wide variety of tasks. Be precise, helpful, and safe.".to_string(),
            max_subagent_depth: 3,
            max_concurrent_subagents: 4,
            subagent_token_budget: 8192,
            max_tool_calls_per_turn: 10,
            max_turns: 100,
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
            plugins_dir: PathBuf::from("plugins"),
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
