# Architecture

## Overview

Unly is a self-hosted personal AI agent platform built as a Rust-first monorepo workspace. It provides a Telegram-first interface to a multi-provider LLM system with long-term memory, tool execution, subagent orchestration, scheduling, plugins, and full audit logging.

## Non-Rust Exceptions

None. The entire platform is implemented in Rust.

## Workspace Structure

```
unly/
├── Cargo.toml                 # Workspace root
├── crates/
│   ├── unly-core/             # Domain model, shared types, traits, errors
│   ├── unly-config/           # Configuration loading/validation
│   ├── unly-db/               # SQLite database access layer + migrations
│   ├── unly-memory/           # Vector memory (SQLite-backed cosine similarity)
│   ├── unly-providers/        # LLM provider abstraction
│   ├── unly-tools/            # Tool execution framework
│   ├── unly-agent/            # Agent runtime + subagent orchestration
│   ├── unly-scheduler/        # Cron/scheduled jobs
│   ├── unly-plugins/          # Plugin SDK and registry
│   ├── unly-audit/            # Append-only audit logging
│   ├── unly-telegram/         # Telegram bot interface
│   └── unly-cli/              # CLI binary (main entrypoint)
├── migrations/                # SQL migrations
├── deploy/                    # systemd unit, env template
├── plugins/
│   └── example/               # Example plugin
└── docs/                      # Documentation
```

## Crate Responsibilities

### `unly-core`
The domain foundation. No external I/O dependencies.
- Domain types: `AgentId`, `ChatId`, `UserId`, `MessageId`
- Shared model types: `ChatMessage`, `ChatRequest`, `ChatResponse`, `Model`, `EmbeddingRequest/Response`
- Trait definitions: `Provider`, `Tool`
- Permissions model: `UserRole`, `PermissionSet`, `Permission`
- Error types
- Utility types: `HealthReport`, `ExecutionStatus`, `Timestamp`

### `unly-config`
Pure configuration. No async.
- `AppConfig` and all sub-configs with `serde` and `Default` impls
- TOML loading via `figment` with environment variable overrides
- `load_config()` and `default_config()` public API

### `unly-db`
Database access layer. SQLite via `sqlx`.
- Connection pool management
- Runtime migrations from `migrations/` directory
- Typed repository structs (dynamic queries — no compile-time DATABASE_URL required)
- Repos: `ChatRepo`, `UserRepo`, `AuditRepo`, `JobRepo`, `MemoryRepo`

### `unly-memory`
Vector memory subsystem.
- `MemoryStore`: embed-and-store, semantic-retrieve, prune
- Embeddings stored as BLOB (little-endian f32 sequences)
- Cosine similarity computed in Rust (no external vector DB)
- `MemoryScope`: per-user, per-chat, per-agent, per-subagent
- Configurable top_k and similarity threshold

### `unly-providers`
LLM provider abstraction layer.
- `Provider` trait (from `unly-core`): `chat`, `embeddings`, `list_models`, `health`
- `CopilotProvider`: GitHub OAuth Device Flow → Copilot API token → OpenAI-compatible completions
- `OpenAiCompatProvider`: any OpenAI-compatible REST API
- `ProviderRegistry`: named provider map, default provider, health aggregation

### `unly-tools`
Secure tool execution framework.
- `ToolRegistry`: named tool map with allowlist/denylist enforcement
- `ExecutionPolicy`: approval requirements, timeouts, concurrency limits, shell allowlist
- `Tool` trait (from `unly-core`): `schema`, `execute`
- `ToolRisk`: `Safe`, `Privileged`, `Dangerous`
- Built-in tools: `http_get`, `http_post`, `fs_read`, `fs_list`, `git_status`, `git_log`, `shell`
- Tokio semaphore for concurrency control
- Timeout enforcement via `tokio::time::timeout`

### `unly-agent`
Agent runtime and subagent orchestration.
- `AgentRuntime`: agentic loop (receive → plan → tool-call → respond)
- `AgentContext`: per-session state (messages, provider, model, approvals)
- Approval workflow: tools needing approval return `AgentResponse::ApprovalRequired`
- `spawn_subagent`: depth-limited subagent execution with isolated context
- Audit logging integration

### `unly-scheduler`
Job scheduler.
- `Scheduler`: cron-expression-based job dispatcher
- Jobs persist in SQLite (`jobs` + `job_runs` tables)
- `tokio::time::interval` + `cron::Schedule` for firing
- `JobCallback`: async fn signature for job handlers
- Concurrency-limited via semaphore

### `unly-plugins`
Plugin system.
- `Plugin` trait: `manifest`, `init`, `shutdown`, `tools`, `commands`, `jobs`, `on_event`, `execute_command`
- `PluginManifest`: identity, version, permissions, capabilities
- `PluginRegistry`: register, init-all, shutdown-all, dispatch events
- Version compatibility check
- Per-plugin config injection on init

### `unly-audit`
Append-only audit logging.
- `AuditLogger`: async queue → background writer task → SQLite `audit_log` table
- `AuditEvent`: typed event with `event_type`, `subject`, `action`, `outcome`, `details`
- Non-blocking fire-and-forget API (`log`, `success`, `denied`, `failure`)

### `unly-telegram`
Telegram bot interface.
- `TelegramBot`: message dispatcher, command router
- Slash commands: `/start`, `/help`, `/status`, `/models`, `/model`, `/provider`, `/approve`, `/deny`, `/reset`, `/memory`, `/audit`, `/jobs`
- Per-chat `SessionStore` (in-memory, each chat gets isolated `AgentContext`)
- Access control: `is_admin`, `is_allowed` based on configured user ID lists
- Inline keyboard for approval flow
- Message chunking for Telegram's 4096-char limit

### `unly-cli`
CLI binary (`unly`).
- `start`: boot all subsystems + Telegram bot
- `setup`: onboarding wizard
- `validate`: config validation
- `doctor`: diagnostics (database, providers, tools)
- `provider-login copilot`: interactive GitHub device flow
- `provider-status`: health of all providers
- `migrate`: run migrations
- `audit`: show recent audit log
- `memory`: list / prune memory entries
- `job`: list / run / enable / disable jobs
- `plugin`: list / enable / disable plugins
- `init-config`: generate default config.toml

## Data Flow

```
Telegram User → TelegramBot
  → access control check
  → get/create AgentContext
  → AgentRuntime.process(message)
    → build messages (system + history)
    → ProviderRegistry.default_provider().chat(request)
    → if tool_calls:
        → ToolRegistry.execute(tool, args, ctx)
          → policy check (approval?)
          → if ApprovalRequired → return to Telegram
          → else execute tool
        → loop
    → return final text
  → save to database
  → AuditLogger.log(event)
  → update SessionStore
  → send response to user
```

## Security Architecture

See `docs/security.md` for the full threat model.

Key principles:
- All tools have an explicit `ToolRisk` classification
- `Privileged` and `Dangerous` tools require user approval by default
- Shell execution is disabled unless an explicit allowlist is configured
- All tool invocations are logged in the audit trail
- Telegram access control: `admin_user_ids` and `allowed_user_ids` lists
- Secrets never appear in logs (redaction enabled by default)
- SQLite WAL mode for safe concurrent access
- systemd security hardening: `PrivateTmp`, `ProtectSystem`, `NoNewPrivileges`

## Technology Choices

| Component | Technology | Rationale |
|---|---|---|
| Async runtime | tokio | De-facto standard, excellent ecosystem |
| HTTP client | reqwest | Mature, async, HTTPS |
| Telegram | teloxide | Best-maintained Rust Telegram bot library |
| Serialization | serde + serde_json | Universal Rust serialization |
| Config | figment + toml | Layered config with env overrides |
| Database | SQLite via sqlx | Zero-deployment, ACID, WAL mode |
| Vector store | SQLite + Rust cosine | No external sidecar, fully embedded |
| CLI | clap | Industry-standard Rust CLI |
| Logging | tracing + tracing-subscriber | Structured, async-safe |
| Error handling | thiserror + anyhow | Typed errors + ergonomic propagation |
| Cron | cron crate | Parse cron expressions, compute next-fire |
