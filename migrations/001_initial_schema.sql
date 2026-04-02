-- Migration 001: Initial schema
-- Creates all core tables for the unly agent platform.

-- Users table
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY NOT NULL,
    telegram_user_id INTEGER UNIQUE,
    username TEXT,
    display_name TEXT,
    role TEXT NOT NULL DEFAULT 'user',
    permissions TEXT NOT NULL DEFAULT '{}',
    is_blocked INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_users_telegram_user_id ON users(telegram_user_id);

-- Chats table
CREATE TABLE IF NOT EXISTS chats (
    id TEXT PRIMARY KEY NOT NULL,
    telegram_chat_id INTEGER UNIQUE,
    title TEXT,
    system_prompt TEXT,
    provider TEXT,
    model TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_chats_telegram_chat_id ON chats(telegram_chat_id);

-- Messages table
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY NOT NULL,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    user_id TEXT REFERENCES users(id),
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    metadata TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_messages_chat_id ON messages(chat_id);
CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at);

-- Memory entries table (vector store with serialized embeddings)
CREATE TABLE IF NOT EXISTS memory_entries (
    id TEXT PRIMARY KEY NOT NULL,
    scope_type TEXT NOT NULL,  -- 'user', 'chat', 'agent', 'subagent'
    scope_id TEXT NOT NULL,
    content TEXT NOT NULL,
    embedding BLOB NOT NULL,   -- serialized Vec<f32> (little-endian IEEE 754)
    source_type TEXT,          -- 'message', 'file', 'note', 'tool_output'
    source_id TEXT,
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    expires_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_memory_scope ON memory_entries(scope_type, scope_id);
CREATE INDEX IF NOT EXISTS idx_memory_expires ON memory_entries(expires_at) WHERE expires_at IS NOT NULL;

-- Jobs table (scheduled and ad-hoc jobs)
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    job_type TEXT NOT NULL,        -- 'cron', 'webhook', 'adhoc'
    cron_expression TEXT,
    payload TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'pending',
    last_run_at TEXT,
    next_run_at TEXT,
    last_error TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    retry_limit INTEGER NOT NULL DEFAULT 3,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_jobs_enabled ON jobs(enabled);
CREATE INDEX IF NOT EXISTS idx_jobs_next_run_at ON jobs(next_run_at) WHERE next_run_at IS NOT NULL;

-- Job runs table
CREATE TABLE IF NOT EXISTS job_runs (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    output TEXT,
    error TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_job_runs_job_id ON job_runs(job_id);
CREATE INDEX IF NOT EXISTS idx_job_runs_started_at ON job_runs(started_at);

-- Subagents table
CREATE TABLE IF NOT EXISTS subagents (
    id TEXT PRIMARY KEY NOT NULL,
    parent_agent_id TEXT,
    depth INTEGER NOT NULL DEFAULT 0,
    goal TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    provider TEXT,
    model TEXT,
    token_budget INTEGER NOT NULL DEFAULT 8192,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    result TEXT,
    error TEXT,
    chat_id TEXT REFERENCES chats(id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_subagents_parent ON subagents(parent_agent_id);
CREATE INDEX IF NOT EXISTS idx_subagents_status ON subagents(status);

-- Tool invocations (audit trail for tool calls)
CREATE TABLE IF NOT EXISTS tool_invocations (
    id TEXT PRIMARY KEY NOT NULL,
    tool_name TEXT NOT NULL,
    tool_call_id TEXT,
    user_id TEXT,
    chat_id TEXT,
    agent_id TEXT,
    args TEXT NOT NULL DEFAULT '{}',
    result TEXT,
    is_error INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    approved_by TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tool_invocations_tool_name ON tool_invocations(tool_name);
CREATE INDEX IF NOT EXISTS idx_tool_invocations_user_id ON tool_invocations(user_id);
CREATE INDEX IF NOT EXISTS idx_tool_invocations_created_at ON tool_invocations(created_at);

-- Audit log (append-only)
CREATE TABLE IF NOT EXISTS audit_log (
    id TEXT PRIMARY KEY NOT NULL,
    event_type TEXT NOT NULL,
    user_id TEXT,
    chat_id TEXT,
    agent_id TEXT,
    subject TEXT NOT NULL,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL,  -- 'success', 'failure', 'denied'
    details TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_log_event_type ON audit_log(event_type);
CREATE INDEX IF NOT EXISTS idx_audit_log_user_id ON audit_log(user_id);
CREATE INDEX IF NOT EXISTS idx_audit_log_created_at ON audit_log(created_at);

-- Webhooks table
CREATE TABLE IF NOT EXISTS webhooks (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    endpoint TEXT NOT NULL,
    secret TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    job_id TEXT REFERENCES jobs(id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Plugin state table
CREATE TABLE IF NOT EXISTS plugin_state (
    plugin_id TEXT PRIMARY KEY NOT NULL,
    version TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    config TEXT NOT NULL DEFAULT '{}',
    installed_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Configuration metadata (versioned config tracking)
CREATE TABLE IF NOT EXISTS config_metadata (
    id TEXT PRIMARY KEY NOT NULL,
    key TEXT NOT NULL UNIQUE,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
