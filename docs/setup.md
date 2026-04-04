# Setup Guide

## Prerequisites

- Linux (Ubuntu 22.04+ recommended)
- Rust 1.75+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- A Telegram bot token (from [@BotFather](https://t.me/BotFather))
- Your Telegram user ID (from [@userinfobot](https://t.me/userinfobot))
- A GitHub account with GitHub Copilot access

---

## Quick Start (Development)

For end-user installation, you can use:

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/oblivisheee/unly/main/install.sh)
```

The installer first tries GitHub Release binaries and automatically falls back to source build when no matching release asset exists.

### 1. Clone and build

```bash
git clone https://github.com/oblivisheee/unly
cd unly
cargo build --release
```

### 2. Generate the default config

```bash
./target/release/unly init-config
```

This creates `config.toml` in the current directory. Edit it:

```toml
[telegram]
bot_token = "YOUR_BOT_TOKEN_FROM_BOTFATHER"
admin_user_ids = [YOUR_TELEGRAM_USER_ID]   # e.g. 123456789
open_access = false
```

### 3. Authenticate with GitHub Copilot

```bash
./target/release/unly provider-login copilot
```

Follow the device flow instructions (open URL, enter code). The token is cached at `data/github_token.json`.

### 4. Validate configuration

```bash
./target/release/unly validate
```

### 5. Run diagnostics

```bash
./target/release/unly doctor
```

### 6. Start the bot

```bash
./target/release/unly start
```

---

## Configuration Reference

The `config.toml` file supports the following sections:

### `[telegram]`
| Key | Type | Default | Description |
|---|---|---|---|
| `bot_token` | string | `""` | Telegram bot token. Also: `TELEGRAM_BOT_TOKEN` env var |
| `admin_user_ids` | array of int | `[]` | Telegram user IDs with admin access |
| `allowed_user_ids` | array of int | `[]` | Telegram user IDs with basic access (empty + open_access=false = nobody) |
| `open_access` | bool | `false` | Allow any Telegram user |
| `context_window_size` | int | `20` | Max messages per chat to keep in context |

### `[database]`
| Key | Type | Default | Description |
|---|---|---|---|
| `path` | path | `data/unly.sqlite` | SQLite database file path |
| `max_connections` | int | `5` | Connection pool size |
| `journal_mode` | string | `WAL` | SQLite journal mode |
| `auto_migrate` | bool | `true` | Run migrations on startup |

### `[providers]`
| Key | Type | Default | Description |
|---|---|---|---|
| `default_provider` | string | `copilot` | Active provider name |
| `default_model` | string | `gpt-4o` | Active model name |

### `[providers.copilot]`
| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Whether Copilot is enabled |
| `github_client_id` | string | (built-in) | GitHub OAuth app client ID |
| `token_cache_path` | path | `data/github_token.json` | Where to cache the auth token |

### `[[providers.openai_compatible]]`
Add one block per additional provider:
```toml
[[providers.openai_compatible]]
name = "openai"
base_url = "https://api.openai.com/v1"
api_key = "sk-..."   # or use env var
models = []          # empty = auto-discover via /models
enabled = true
```

### `[tools]`
| Key | Type | Default | Description |
|---|---|---|---|
| `enabled_tools` | array | see defaults | Tool names to enable (empty = all non-dangerous) |
| `disabled_tools` | array | `[]` | Tool names to block |
| `require_approval_for_privileged` | bool | `true` | Require /approve for privileged tools |
| `require_approval_for_dangerous` | bool | `true` | Require /approve for dangerous tools |
| `max_execution_seconds` | int | `30` | Tool timeout |
| `shell_allowlist` | array | `[]` | Regex patterns for allowed shell commands |

### `[agent]`
| Key | Type | Default | Description |
|---|---|---|---|
| `system_prompt` | string | (built-in) | System prompt for every conversation |
| `max_tool_calls_per_turn` | int | `10` | Max tool calls per agent turn |
| `max_turns` | int | `100` | Max turns before reset |

### `[logging]`
| Key | Type | Default | Description |
|---|---|---|---|
| `level` | string | `info` | Log level (trace/debug/info/warn/error) |
| `json` | bool | `false` | JSON-structured log output |

---

## Environment Variable Overrides

All sensitive values can be passed as environment variables (they override config.toml):

| Variable | Config key |
|---|---|
| `TELEGRAM_BOT_TOKEN` | `telegram.bot_token` |
| `RUST_LOG` | log level |

---

## Adding an OpenAI-Compatible Provider

Example for a local Ollama instance:

```toml
[[providers.openai_compatible]]
name = "ollama"
base_url = "http://localhost:11434/v1"
api_key = "unused"
models = ["llama3", "mistral"]
enabled = true
```

Switch to it in chat:
```
/provider ollama
/model llama3
```

---

## Directory Layout (Runtime)

```
data/
  unly.sqlite          # SQLite database
  github_token.json    # Cached GitHub OAuth token (mode 600)
config.toml            # Main configuration
```
