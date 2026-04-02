# unly

**Self-hosted personal AI agent, accessible via Telegram.**

Unly is a Rust-based platform that turns any Telegram chat into an agentic AI assistant. It connects to GitHub Copilot (or any OpenAI-compatible API), maintains long-term semantic memory, executes tools with a built-in approval workflow, and runs entirely on your own infrastructure.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

---

## Features

- **Telegram-first interface** — talk to your agent through any Telegram client
- **Semantic memory** — remembers past conversations using vector similarity search (SQLite-backed, no external DB)
- **Multi-provider LLM** — GitHub Copilot out of the box, plus any OpenAI-compatible API (OpenAI, Ollama, etc.)
- **Agentic tool execution** — HTTP requests, file I/O, git inspection, shell commands
- **Approval workflow** — privileged and dangerous tools require explicit `/approve` before execution
- **Security by default** — allowlist-based access control, audit trail, secret redaction, shell disabled unless configured
- **Job scheduler** — cron-based background tasks
- **Plugin system** — extend capabilities with your own Rust plugins
- **Append-only audit log** — every tool call, approval, and denial is recorded

---

## Install

Run the installer — it handles Rust, the build, and the onboarding wizard in one step:

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

After the build completes, the onboarding wizard (`unly setup`) will guide you through the remaining steps.

---

## Configuration

The `config.toml` file controls every aspect of the agent. Below are the most important sections.

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
enabled_tools                  = []     # empty = all non-dangerous tools
disabled_tools                 = []
require_approval_for_privileged = true  # http_post, etc. need /approve
require_approval_for_dangerous  = true  # shell needs /approve
max_execution_seconds          = 30
shell_allowlist                = []     # empty = shell disabled entirely
```

**Built-in tools and their risk level:**

| Tool | Risk | Description |
|------|------|-------------|
| `http_get` | Safe | HTTP GET request |
| `http_post` | Privileged | HTTP POST request |
| `fs_read` | Safe | Read a file |
| `fs_list` | Safe | List directory contents |
| `git_status` | Safe | `git status` output |
| `git_log` | Safe | `git log` output |
| `shell` | Dangerous | Execute a shell command |

### Agent behaviour

```toml
[agent]
system_prompt          = "You are a helpful personal assistant."
max_tool_calls_per_turn = 10
max_turns              = 100
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

---

## Telegram Bot Commands

| Command | Access | Description |
|---------|--------|-------------|
| `/start` | All | Begin a new session |
| `/help` | All | Show available commands |
| `/status` | All | Current provider, model, and session info |
| `/models` | All | List available models |
| `/model <name>` | All | Switch to a different model |
| `/provider <name>` | All | Switch to a different provider |
| `/approve` | All | Approve a pending tool call |
| `/deny` | All | Deny a pending tool call |
| `/reset` | All | Clear conversation context |
| `/memory list` | All | Show stored memory entries |
| `/memory prune` | All | Delete old memory entries |
| `/audit` | Admin | Show recent audit log |
| `/jobs` | Admin | List scheduled jobs |
| `/plugin` | Admin | List installed plugins |

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
data/
  unly.sqlite            # SQLite database (WAL mode)
  github_token.json      # Cached GitHub OAuth token (mode 600)
config.toml              # Main configuration
```

---

## License

[MIT](LICENSE)
