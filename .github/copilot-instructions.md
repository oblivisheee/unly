# Copilot Instructions for `unly`

## Build, test, and lint commands

Run from repository root.

```bash
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Lint (warnings are treated as errors in this repo's check flow)
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --all -- --check
```

Single-test patterns used in this workspace:

```bash
# Single integration test function
cargo test -p unly-config --test config_tests default_config_is_valid_structure -- --exact

# Single unit/integration test by name in another crate
cargo test -p unly-memory serialization_roundtrip_preserves_values -- --exact
```

## High-level architecture

`unly` is a Rust workspace where `unly-cli` wires together all runtime components. The `start` path in `crates/unly-cli/src/commands.rs` and service construction in `crates/unly-cli/src/service.rs` are the canonical assembly points: config load → database connection/migrations → provider registry → tool registry/policy → agent runtime → Telegram bot loop.

Message flow is: Telegram update (`unly-telegram`) → access check/session lookup → `AgentRuntime` (`unly-agent`) conversation loop → provider chat call (`unly-providers`) with optional tool calls → tool execution (`unly-tools`) with policy/approval gates → result back into model loop → response persisted/audited (`unly-db`, `unly-audit`) and sent to Telegram.

Core crate roles:
- `unly-core`: shared domain types/traits/errors used everywhere.
- `unly-config`: typed config + env overlay and validation.
- `unly-db`: SeaORM-backed persistence and migrations.
- `unly-agent`: turn loop, tool-call loop, approval handling, streaming.
- `unly-tools`: tool registry, risk policy, timeout/concurrency enforcement.
- `unly-providers`: Copilot + OpenAI-compatible provider abstraction.
- `unly-telegram`: command router, RBAC gate, per-chat session store.

## Key repository conventions

- **Workspace-home-first runtime paths**: defaults resolve under `UNLY_HOME` (or `~/.unly`) via `unly-config::workspace` (`config.toml`, `data/unly.sqlite`, token cache, `IDENTITY.md`, `BOOT.md`).
- **System prompt composition is file-backed**: runtime prompt is loaded from `IDENTITY.md` + `BOOT.md`; defaults are auto-written if missing (`build_runtime` / `load_system_prompt` in `unly-cli` service wiring).
- **Tool exposure is policy-driven, not hardcoded trust**: tools are always registered through `ToolRegistry`; access is the intersection of enabled/disabled lists plus risk policy checks (`Safe` / `Privileged` / `Dangerous`) and approval state.
- **Shell tool is opt-in by allowlist**: shell is only registered when `shell_allowlist` is non-empty; empty allowlist means no shell tool exposure.
- **Approval is a first-class control path**: denied privileged/dangerous tool calls become pending approvals; `/approve` and `/deny` in Telegram continue or cancel the exact queued calls.
- **Access control is Telegram-ID based allowlisting**: admin IDs and allowed IDs are checked per request, with `open_access` as explicit override.
- **Config loading model**: file-based TOML is merged with `UNLY_` env overrides (`__` section separator), plus specific convenience vars like `TELEGRAM_BOT_TOKEN`.
- **Database backend is selectable**: SQLite default with WAL pragmas and migration-on-start behavior; PostgreSQL support is wired through the same `DatabaseConfig`.

