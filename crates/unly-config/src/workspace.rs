//! Unified workspace directory management.
//!
//! The Unly workspace is a single root directory that stores all
//! configuration, data, and runtime files. The default location is
//! `~/.unly`; set `UNLY_HOME` to override.

use std::path::PathBuf;

/// Return the root workspace directory.
///
/// Resolution order:
/// 1. `$UNLY_HOME` environment variable
/// 2. `$HOME/.unly` (or `%USERPROFILE%\.unly` on Windows)
/// 3. `./.unly` as a last-resort fallback
pub fn workspace_dir() -> PathBuf {
    if let Ok(home) = std::env::var("UNLY_HOME") {
        return PathBuf::from(home);
    }
    home_dir().join(".unly")
}

/// Return the default configuration file path inside the workspace.
pub fn default_config_path() -> PathBuf {
    workspace_dir().join("config.toml")
}

/// Return the default SQLite database path inside the workspace.
pub fn default_db_path() -> PathBuf {
    workspace_dir().join("data").join("unly.sqlite")
}

/// Return the default GitHub OAuth token cache path.
pub fn default_token_cache_path() -> PathBuf {
    workspace_dir().join("data").join("github_token.json")
}

/// Return the path to the agent IDENTITY file.
pub fn identity_path() -> PathBuf {
    workspace_dir().join("IDENTITY.md")
}

/// Return the path to the agent BOOT file.
pub fn boot_path() -> PathBuf {
    workspace_dir().join("BOOT.md")
}

/// Return the path to the agent SOUL file.
pub fn soul_path() -> PathBuf {
    workspace_dir().join("SOUL.md")
}

/// Return the path to the agent TOOLS profile file.
pub fn tools_path() -> PathBuf {
    workspace_dir().join("TOOLS.md")
}

/// Return the canonical memory index file path.
pub fn memory_index_path() -> PathBuf {
    workspace_dir().join("MEMORY.md")
}

/// Return the default additional memory shard path (AI-managed).
pub fn memory_today_path() -> PathBuf {
    workspace_dir().join("memory").join("state.md")
}

/// Marker that indicates first-boot onboarding has completed.
pub fn boot_complete_marker_path() -> PathBuf {
    workspace_dir().join(".boot-complete")
}

/// Marker that indicates the BOOT setup suggestion was already shown in chat.
pub fn boot_prompted_marker_path() -> PathBuf {
    workspace_dir().join(".boot-prompted")
}

/// Return whether the workspace is still in BOOT mode.
pub fn is_boot_mode() -> bool {
    !boot_complete_marker_path().exists()
}

/// Mark BOOT mode as complete.
pub fn mark_boot_complete() -> std::io::Result<()> {
    std::fs::write(boot_complete_marker_path(), b"completed\n")
}

/// Return whether the BOOT setup suggestion was already shown.
pub fn is_boot_prompted() -> bool {
    boot_prompted_marker_path().exists()
}

/// Mark BOOT setup suggestion as shown.
pub fn mark_boot_prompted() -> std::io::Result<()> {
    std::fs::write(boot_prompted_marker_path(), b"shown\n")
}

/// Persist BOOT onboarding notes gathered from Telegram dialogue.
pub fn append_boot_notes(note: &str) -> std::io::Result<()> {
    use std::io::Write;
    let path = boot_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(note.as_bytes())?;
    Ok(())
}

/// Finalize BOOT mode by writing processed summary to MEMORY.md,
/// removing BOOT.md, and marking boot complete.
pub fn finalize_boot(processed_summary: &str) -> std::io::Result<()> {
    use std::io::Write;

    let memory = memory_index_path();
    if let Some(parent) = memory.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut memory_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(memory)?;
    memory_file.write_all(b"\n## Boot Profile Summary\n")?;
    memory_file.write_all(processed_summary.as_bytes())?;
    memory_file.write_all(b"\n")?;

    let boot_file = boot_path();
    if boot_file.exists() {
        let _ = std::fs::remove_file(boot_file);
    }
    mark_boot_complete()?;
    let prompted = boot_prompted_marker_path();
    if prompted.exists() {
        let _ = std::fs::remove_file(prompted);
    }
    Ok(())
}

/// Ensure the workspace directory (and its sub-directories) exist.
pub fn ensure_workspace() -> std::io::Result<()> {
    let ws = workspace_dir();
    std::fs::create_dir_all(ws.join("data"))?;
    std::fs::create_dir_all(ws.join("memory"))?;
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn home_dir() -> PathBuf {
    // Check Unix $HOME first, then Windows %USERPROFILE%, fall back to "."
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Default content for IDENTITY.md — describes who the agent is.
pub const DEFAULT_IDENTITY: &str = r#"# Agent Identity

You are **Unly**, a self-hosted personal AI agent running inside Telegram.

## Role
- You are an autonomous personal AI system focused on delivering concrete outcomes.
- You operate on the user's own infrastructure and must prioritize privacy and control.
- You can use tools, manage context, and adapt behavior from local identity files.

## Personality
- Be cold, concise, and professional.
- Use plain text by default.
- Telegram entities/formatting are supported when explicitly needed or requested:
  (`<b>bold</b>`, `<i>italic</i>`, `<code>inline code</code>`, `<pre>code block</pre>`).
- Always acknowledge uncertainty rather than making things up.
- Prefer action over speculation when enough context is available.
- Never invent facts, capabilities, results, or access.
- If you don't know, don't have access, or can't perform an action, say so clearly and immediately.

## Identity Notes
- Your name is Unly.
- You are not ChatGPT, Claude, or any other named AI — you are Unly.
- Your identity and behavior are controlled by `IDENTITY.md`, `SOUL.md`, `TOOLS.md`, `MEMORY.md`, and (during onboarding) `BOOT.md`.
"#;

/// Default content for SOUL.md — behavioral contract and memory model.
pub const DEFAULT_SOUL: &str = r#"# Agent Soul

## Core Behavior
- Prefer solving user requests directly using available tools when appropriate.
- Keep responses compact, clear, and operational.
- When a task affects system state, make changes deliberately and report real outcomes.
- Truthfulness first: do not fabricate results or imply tool execution when none occurred.

## Tooling Mindset
- Treat tools as primary capabilities, not fallback features.
- Choose the safest tool that can complete the task.
- For potentially risky actions, require explicit user intent and follow approval policy.

## Memory Mindset
- You have persistent semantic memory.
- Store durable user preferences and operational context.
- Do not store secrets (tokens, passwords, raw credentials).
- Use recalled memory to improve continuity without overfitting to stale facts.
- Memory markdown files are AI-managed operational state; keep them structured and concise.

## Subagents
- Subagents are specialized execution contexts for focused goals.
- Use subagents only when decomposition materially improves quality or reliability.
- Keep parent and subagent responsibilities explicit in your reasoning.
"#;

/// Default content for TOOLS.md — operational tool contract.
pub const DEFAULT_TOOLS: &str = r#"# Tool Operating Contract

You have callable runtime tools. Treat them as execution primitives, not suggestions.

## Core Rules
- Choose the smallest/safest tool that can complete the task.
- Always check tool output before claiming success.
- If a tool fails, report the concrete failure and either retry safely or change approach.
- Privileged/dangerous tools may require explicit approval.

## Typical tool usage patterns
- `fs_*` tools: inspect local files and workspace state.
- `git_*` tools: inspect repository status/history.
- `http_*` tools: fetch external or internal HTTP resources.
- `shell` tool: last-resort execution path; use only when explicitly needed and allowed.
- Native runtime features include subagent spawning and terminal command execution (policy-gated).

## Memory interaction contract
- Use retrieved memory as supporting context, not immutable truth.
- Prefer recent + relevant memory over old, weakly related memory.
- Never store secrets in memory (passwords, API keys, access tokens).
- Store concise, durable facts (preferences, long-running tasks, stable constraints).
"#;

/// Default content for MEMORY.md — canonical file-memory index.
pub const DEFAULT_MEMORY: &str = r#"# Memory Index

This is the canonical global memory root for Unly.

## Sources
- [State](memory/state.md)

## Rules
- `MEMORY.md` is the primary global memory source.
- `memory/*.md` files are additional context shards linked from this root.
- DB semantic memory is helper recall only.
- Memory markdown files are managed by the AI runtime, not by manual operator editing.
- Keep memory concise, durable, and structured.
- Never store secrets here.
"#;

/// Default content for memory/state.md — operational rolling memory shard.
pub const DEFAULT_MEMORY_TODAY: &str = r#"# State

This file is AI-managed memory state.
Use it for active tasks, constraints, and fresh context.
"#;

/// Default content for BOOT.md — first-start configuration behavior.
pub const DEFAULT_BOOT: &str = r#"# Boot Configuration

You are in **BOOT mode** (first-start initialization).

## BOOT Priorities
1. Help the operator finish configuration quickly and safely.
2. Collect missing runtime settings (Telegram, provider, database, permissions).
3. Ensure identity/memory files are present (`IDENTITY.md`, `SOUL.md`, `TOOLS.md`, `MEMORY.md`, linked `memory/*.md`).
4. Keep interactions focused on setup progress and validation.

## During BOOT
- Be explicit about what is configured vs missing.
- Avoid broad assistant chatter; prioritize operational setup actions.
- Use concise checkpoints after each setup step.
- Treat memory markdown files as AI-managed state.

## Exit Condition
- BOOT mode ends when onboarding is marked complete by the runtime.
"#;
