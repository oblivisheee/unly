# Security Model

## Principles

- **Least privilege everywhere**: every component only has the access it needs
- **Explicit capability grants**: sensitive tools must be explicitly enabled
- **Defense in depth**: multiple layers of checks before any privileged action
- **Append-only audit trail**: all security-relevant events are logged and cannot be modified
- **Secret isolation**: secrets never appear in source control or logs
- **Secure-by-default**: restrictive defaults that must be explicitly relaxed

---

## Threat Model

### Telegram Bot Access Control

| Threat | Mitigation |
|---|---|
| Unauthorized user sends messages | `admin_user_ids` and `allowed_user_ids` allowlists; `open_access = false` by default |
| Admin impersonation | Telegram user IDs are enforced at the bot layer before any action |
| Blocked user retakes access | Blocked users are re-checked on every request |

### Tool Execution

| Threat | Mitigation |
|---|---|
| Unrestricted command execution | No shell execution without explicit `shell_allowlist` |
| Privilege escalation via shell | Shell executed with `env_clear()`, restricted `$PATH`, no ambient capabilities |
| Tool SSRF/exfiltration | HTTP tools respect only http/https; no internal network bypass |
| Path traversal in filesystem tools | `..` components rejected before any filesystem access |
| Runaway tool execution | Per-tool timeout via `tokio::time::timeout` + concurrency semaphore |
| Missing approval for privileged actions | `require_approval_for_privileged = true` by default; agent runtime returns `ApprovalRequired` before execution |

### LLM Provider

| Threat | Mitigation |
|---|---|
| Token leakage | Tokens cached with mode 600 permissions on disk; never echoed in logs |
| Prompt injection via tool output | Tool outputs are treated as data (role: tool), not instructions |
| Provider auth failure | Auth errors surfaced to admin, bot gracefully degrades |

### Plugin System

| Threat | Mitigation |
|---|---|
| Malicious plugin | Plugins are in-process Rust code — no arbitrary code loading; all plugins must be compiled in |
| Plugin accessing unauthorized data | Plugin permissions declared in manifest; registry validates before registration |
| Plugin crashing the service | Plugin lifecycle errors are caught and logged; they don't panic the runtime |

### Database

| Threat | Mitigation |
|---|---|
| Data corruption on crash | SQLite WAL mode provides atomic writes |
| SQL injection | All queries use parameterized bindings via sqlx |
| Unauthorized database access | SQLite file has restrictive permissions; service runs as unprivileged `unly` user |

### Audit Log

| Threat | Mitigation |
|---|---|
| Event suppression | Audit events are queued and written asynchronously; failures are logged to stderr |
| Log tampering | Append-only design; no UPDATE/DELETE on audit_log table in normal operation |
| Missing critical events | All tool executions, approvals, denials, and login attempts are audited |

### systemd Hardening

The service unit includes:
- `NoNewPrivileges=true`
- `PrivateTmp=true`
- `ProtectSystem=strict`
- `ProtectHome=read-only`
- `CapabilityBoundingSet=` (empty = no capabilities)
- `MemoryMax=512M` (prevents runaway allocation)

---

## RBAC

Two roles are implemented:
- **admin**: set via `telegram.admin_user_ids`; can use all commands including `/audit`, `/memory`, `/jobs`
- **user**: set via `telegram.allowed_user_ids`; can chat and use approved tools

`PermissionSet` values:
- `chat` (all users)
- `use_tools` (all users — subject to policy approval)
- `approve_tools` (all users — approve their own pending actions)
- `view_audit` (admin only)
- `manage_jobs` (admin only)
- `manage_plugins` (admin only)

---

## Secret Management Rules

1. **Never hardcode secrets**. All secrets are in environment variables or `/etc/unly/unly.env` (mode 600)
2. **Never log secrets**. `redact_secrets = true` by default
3. **Never version-control secrets**. `.gitignore` excludes `*.env`, `*_token.json`, `unly.sqlite`
4. **Never echo secrets in Telegram responses**. The bot redacts provider configuration in all outputs

---

## Recommended Production Checklist

- [ ] `open_access = false`
- [ ] `admin_user_ids` contains only your own user ID
- [ ] `allowed_user_ids` is minimal
- [ ] `shell_allowlist` is empty unless you need it
- [ ] `require_approval_for_privileged = true`
- [ ] `require_approval_for_dangerous = true`
- [ ] Database file is not world-readable
- [ ] `/etc/unly/unly.env` is mode 600
- [ ] Webhook secret is set if webhooks are enabled
- [ ] Logs are reviewed periodically
- [ ] `unly doctor` passes on startup
