# unly

**Self-hosted personal AI agent, accessible via Telegram.**

Unly is a Rust-based platform that turns any Telegram chat into an agentic AI assistant. It connects to GitHub Copilot (or any OpenAI-compatible API), maintains long-term semantic memory, executes tools with a built-in approval workflow, and runs entirely on your own infrastructure.

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL%20v3-blue.svg)](LICENSE)

---

## Features

- **Telegram-first interface** — talk to your agent through any Telegram client
- **Semantic memory** — remembers past conversations using vector similarity search (SQLite-backed, no external DB)
- **Multi-provider LLM** — GitHub Copilot out of the box, plus any OpenAI-compatible API (OpenAI, Ollama, etc.)
- **Agentic tool execution** — HTTP, extended file tools, git inspection, shell/bash, subagents, cron jobs
- **Approval workflow** — manual approvals by default, with global `/approval auto|manual` and setup-time full-access mode
- **Security by default** — allowlist-based access control, audit trail, secret redaction, and policy-gated tool risks
- **Job scheduler** — cron-based background tasks
- **Plugin system** — extend capabilities with your own Rust plugins
- **Append-only audit log** — every tool call, approval, and denial is recorded
- **Workspace-home runtime** — config/data/identity live under `UNLY_HOME` (default `~/.unly`)

---

## Install

Run the installer — it handles installation and onboarding in one step:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/oblivisheee/unly/main/install.sh)
```

Or, from a local clone:

```bash
bash install.sh
```

Before running, you will need:

- Linux (Ubuntu 22.04+ recommended)
- A Telegram bot token — create one with [@BotFather](https://t.me/BotFather)
- Your Telegram user ID — get it from [@userinfobot](https://t.me/userinfobot)
- A GitHub account with GitHub Copilot access *(or any OpenAI-compatible API key)*

Installer behavior:
- Tries to download a prebuilt binary from the latest GitHub Release.
- Falls back to source build automatically if no matching release asset is available.

After installation, the onboarding wizard (`unly setup`) will guide you through the remaining steps.
It now fetches available models from the selected provider and lets you pick a default model from the fetched list (with manual fallback if listing fails).

## CLI Commands

Common commands:

```bash
unly setup
unly start
unly validate
unly doctor
unly service install
unly service status
unly uninstall
unly uninstall-cli
unly update --check
unly update
```

Update command notes:
- `unly update --check` — check if a newer release exists.
- `unly update` — download and install the latest release binary.
- `unly update --repo owner/repo` — check/install from a specific GitHub repository.
- You can also set `UNLY_RELEASE_REPO=owner/repo`.

Service command notes:
- `unly service install` — install bundled `systemd` unit for Unly.
- `unly service enable|disable|start|stop|restart|status` — manage service lifecycle.
- Service commands auto-elevate with `sudo` when needed (you will be prompted for your root password).

Uninstall command notes:
- `unly uninstall` — remove workspace/config data.
- `unly uninstall-cli` — remove CLI binary and installed systemd service (if present).

---

## Configuration

The `config.toml` file controls every aspect of the agent. Below are the most important sections.

### Workspace paths

By default, runtime files are stored in `~/.unly` (override with `UNLY_HOME`):

```text
~/.unly/
  config.toml
  data/unly.sqlite
  data/github_token.json
  IDENTITY.md
  SOUL.md
  TOOLS.md
  MEMORY.md
  memory/state.md
```

### Telegram access control

```toml
[telegram]
bot_token = ""                  # BotFather token (or TELEGRAM_BOT_TOKEN env var)
admin_user_ids = [123456789]    # Full admin access
allowed_user_ids = []           # Regular user access
open_access = false             # Set true to allow any Telegram user
context_window_size = 20        # Number of messages kept in context
```

### Providers

```toml
[providers]
default_provider = "copilot"
default_model    = "gpt-4o"

[providers.copilot]
enabled = true
# token is cached automatically by `provider-login copilot`

# --- OR use any OpenAI-compatible API ---
[[providers.openai_compatible]]
name     = "openai"
base_url = "https://api.openai.com/v1"
api_key  = "sk-..."    # can also be an env var
enabled  = true
```

#### Local models with Ollama

```toml
[[providers.openai_compatible]]
name     = "ollama"
base_url = "http://localhost:11434/v1"
api_key  = "not-required"   # Ollama does not require authentication
models   = ["llama3", "mistral"]
enabled  = true
```

Switch to it in Telegram chat:
```
/provider ollama
/model llama3
```

### Tool execution policy

```toml
[tools]
enabled_tools                   = []      # empty = all registered tools except explicitly disabled
disabled_tools                  = []
require_approval_for_privileged = true    # set false to execute privileged tools immediately
require_approval_for_dangerous  = true    # set false to execute dangerous tools immediately
max_execution_seconds           = 30
max_concurrent_executions       = 4
shell_allowlist                 = ["^ls(\\s|$)", "^pwd(\\s|$)", "^cat\\s+", "^echo\\s+"]
# set shell_allowlist = [] to disable shell/bash execution
```

Approval behavior:
- `/approval manual` keeps explicit approve/deny flow for pending tool actions.
- `/approval auto` auto-approves pending tool actions globally.
- During `unly setup`, selecting full access disables approval prompts globally in config.
- Manual/auto applies to tool execution flow, not to plain chat text.

**Built-in tools and their risk level:**

| Tool | Risk | Description |
|------|------|-------------|
| `http_get` | Safe | HTTP GET request |
| `http_post` | Privileged | HTTP POST request |
| `fs_read` | Safe | Read a file |
| `fs_list` | Safe | List directory contents |
| `fs_write` | Privileged | Write/append text to a file |
| `fs_delete` | Dangerous | Delete file or directory |
| `fs_copy` | Privileged | Copy file or directory |
| `fs_move` | Privileged | Move/rename file or directory |
| `fs_mkdir` | Privileged | Create directories |
| `fs_stat` | Safe | Show file metadata |
| `fs_grep` | Safe | Search text in files |
| `git_status` | Safe | `git status` output |
| `git_log` | Safe | `git log` output |
| `shell` / `bash` | Dangerous | Execute shell commands (allowlist-checked) |
| `spawn_subagent` | Privileged | Delegate a task to a subagent (use only when user explicitly asks for delegation) |
| `cron_job` | Privileged | Manage scheduled background jobs |

### Agent behaviour

```toml
[agent]
max_subagent_depth      = 3
max_concurrent_subagents = 4
max_tool_calls_per_turn = 10
max_turns               = 100
use_file_memory_primary = true
```

### Semantic memory

```toml
[memory]
enabled              = true
embedding_provider   = "copilot"
embedding_model      = "text-embedding-3-small"
top_k                = 5     # results returned per memory query
similarity_threshold = 0.7
raw_retention_days   = 90    # purge raw messages after 90 days
memory_retention_days = 0    # 0 = keep memories forever
```

For the full configuration reference, see [docs/setup.md](docs/setup.md).

### Plugins

```toml
[plugins]
plugins_dir = "~/.unly/plugins"
allow_unknown = false
enabled = ["com.unly.random-fortune"]  # include to force-enable
disabled = []                          # add plugin IDs here to disable
```

Built-in plugin currently wired into runtime:
- `com.unly.random-fortune` — adds Telegram command `/fortune`

---

## Telegram Bot Commands

| Command | Access | Description |
|---------|--------|-------------|
| `/start` | All | Begin a new session |
| `/new` | All | Begin a new session |
| `/help` | All | Show available commands |
| `/status` | All | Current provider, model, and session info |
| `/model <name>` | All | Switch to a different model |
| `/provider <name>` | All | Switch to a different provider |
| `/subagents` | All | Show active subagents and statuses |
| `/spawn_subagent <task>` | All | Explicitly request a delegated subagent task |
| `/approve` | All | Approve pending tool calls (manual mode) |
| `/deny` | All | Deny pending tool calls (manual mode) |
| `/approval <manual\\|auto>` | All | Switch global approval mode |
| `/reset` | All | Clear conversation context |

---

## Architecture Overview

Unly is a Rust workspace with 12 specialised crates:

```
unly-cli  (binary)
├── unly-telegram      Telegram bot, command routing, session management
├── unly-agent         Agentic loop, subagent orchestration, approval workflow
├── unly-providers     LLM provider abstraction (Copilot, OpenAI-compatible)
├── unly-tools         Tool registry, execution policy, built-in tools
├── unly-memory        Semantic memory (cosine similarity over SQLite)
├── unly-scheduler     Cron-based background job scheduler
├── unly-plugins       Plugin trait, registry, lifecycle hooks
├── unly-audit         Append-only audit logger
├── unly-db            SQLite access layer + migrations
├── unly-config        TOML config loading with env overrides
└── unly-core          Domain types, traits, errors (no I/O)
```

### Release and self-update

Repository includes a GitHub Actions workflow for mainline releases:
- `.github/workflows/release-main.yml`
- Trigger: `push` to `main` and manual `workflow_dispatch`
- Builds release binaries for:
  - `x86_64-unknown-linux-gnu`
  - `aarch64-apple-darwin`
- Publishes GitHub Release assets consumed by `unly update`

**Message flow:**

```
Telegram user
  → access control check
  → AgentRuntime.process(message)
      → LLM chat (Copilot / OpenAI-compatible)
      → if tool_calls → ToolRegistry.execute()
          → policy check (approval required?)
          → execute / return ApprovalRequired
      → loop until final text
  → persist to SQLite
  → AuditLogger.log(event)
  → send reply to user
```

Telegram formatting:
- Messages are sent in plain text by default.
- Short assistant responses are also attempted with Telegram HTML entity parsing.
- If parsing fails, delivery falls back to plain text automatically.

For a deeper dive, see:
- [docs/architecture.md](docs/architecture.md) — full crate descriptions and technology choices
- [docs/security.md](docs/security.md) — threat model, RBAC, secret handling
- [docs/deployment.md](docs/deployment.md) — production deployment with systemd
- [docs/setup.md](docs/setup.md) — complete configuration reference

---

## Production Deployment

A ready-made systemd unit file is provided in [`deploy/`](deploy/). It includes security hardening (`NoNewPrivileges`, `PrivateTmp`, `ProtectSystem`, memory limits). See [docs/deployment.md](docs/deployment.md) for full instructions.

---

## Data Layout

After the first run, Unly creates the following files:

```
$UNLY_HOME (default: ~/.unly)/
  config.toml              # Main configuration
  data/unly.sqlite         # SQLite database (WAL mode)
  data/github_token.json   # Cached GitHub OAuth token (mode 600)
  IDENTITY.md              # Core runtime identity prompt
  SOUL.md                  # Runtime behavior contract
  TOOLS.md                 # Tool-use contract
  MEMORY.md                # Canonical memory index
  memory/state.md          # Rolling memory shard
```

---

## License

[GPL-3.0](LICENSE)
