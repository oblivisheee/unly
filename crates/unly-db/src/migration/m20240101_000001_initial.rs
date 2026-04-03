use sea_orm_migration::prelude::*;

/// Initial schema migration — creates all core tables.
pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20240101_000001_initial"
    }
}

/// Table / column name identifiers used by the schema builder.
#[derive(Iden)]
enum Users {
    Table,
    Id,
    TelegramUserId,
    Username,
    DisplayName,
    Role,
    Permissions,
    IsBlocked,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Chats {
    Table,
    Id,
    TelegramChatId,
    Title,
    SystemPrompt,
    Provider,
    Model,
    CreatedAt,
    UpdatedAt,
    Metadata,
}

#[derive(Iden)]
enum Messages {
    Table,
    Id,
    ChatId,
    UserId,
    Role,
    Content,
    CreatedAt,
    Metadata,
}

#[derive(Iden)]
enum MemoryEntries {
    Table,
    Id,
    ScopeType,
    ScopeId,
    Content,
    Embedding,
    SourceType,
    SourceId,
    Metadata,
    CreatedAt,
    ExpiresAt,
}

#[derive(Iden)]
enum Jobs {
    Table,
    Id,
    Name,
    Description,
    JobType,
    CronExpression,
    Payload,
    Status,
    LastRunAt,
    NextRunAt,
    LastError,
    RetryCount,
    RetryLimit,
    Enabled,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum JobRuns {
    Table,
    Id,
    JobId,
    Status,
    Output,
    Error,
    StartedAt,
    FinishedAt,
}

#[derive(Iden)]
enum Subagents {
    Table,
    Id,
    ParentAgentId,
    Depth,
    Goal,
    Status,
    Provider,
    Model,
    TokenBudget,
    TokensUsed,
    Result,
    Error,
    ChatId,
    CreatedAt,
    UpdatedAt,
    FinishedAt,
}

#[derive(Iden)]
enum ToolInvocations {
    Table,
    Id,
    ToolName,
    ToolCallId,
    UserId,
    ChatId,
    AgentId,
    Args,
    Result,
    IsError,
    DurationMs,
    ApprovedBy,
    CreatedAt,
}

#[derive(Iden)]
enum AuditLog {
    Table,
    Id,
    EventType,
    UserId,
    ChatId,
    AgentId,
    Subject,
    Action,
    Outcome,
    Details,
    CreatedAt,
}

#[derive(Iden)]
enum Webhooks {
    Table,
    Id,
    Name,
    Description,
    Endpoint,
    Secret,
    Enabled,
    JobId,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum PluginState {
    Table,
    PluginId,
    Version,
    Enabled,
    Config,
    InstalledAt,
    UpdatedAt,
}

#[derive(Iden)]
enum ConfigMetadata {
    Table,
    Id,
    Key,
    Value,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── users ──────────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Users::Id).string().not_null().primary_key())
                    .col(
                        ColumnDef::new(Users::TelegramUserId)
                            .big_integer()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Users::Username).string())
                    .col(ColumnDef::new(Users::DisplayName).string())
                    .col(
                        ColumnDef::new(Users::Role)
                            .string()
                            .not_null()
                            .default("user"),
                    )
                    .col(
                        ColumnDef::new(Users::Permissions)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(Users::IsBlocked)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Users::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Users::UpdatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_users_telegram_user_id")
                    .table(Users::Table)
                    .col(Users::TelegramUserId)
                    .to_owned(),
            )
            .await?;

        // ── chats ──────────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Chats::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Chats::Id).string().not_null().primary_key())
                    .col(
                        ColumnDef::new(Chats::TelegramChatId)
                            .big_integer()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Chats::Title).string())
                    .col(ColumnDef::new(Chats::SystemPrompt).text())
                    .col(ColumnDef::new(Chats::Provider).string())
                    .col(ColumnDef::new(Chats::Model).string())
                    .col(ColumnDef::new(Chats::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Chats::UpdatedAt).string().not_null())
                    .col(
                        ColumnDef::new(Chats::Metadata)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_chats_telegram_chat_id")
                    .table(Chats::Table)
                    .col(Chats::TelegramChatId)
                    .to_owned(),
            )
            .await?;

        // ── messages ───────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Messages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Messages::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Messages::ChatId).string().not_null())
                    .col(ColumnDef::new(Messages::UserId).string())
                    .col(ColumnDef::new(Messages::Role).string().not_null())
                    .col(ColumnDef::new(Messages::Content).text().not_null())
                    .col(ColumnDef::new(Messages::CreatedAt).string().not_null())
                    .col(
                        ColumnDef::new(Messages::Metadata)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Messages::Table, Messages::ChatId)
                            .to(Chats::Table, Chats::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_messages_chat_id")
                    .table(Messages::Table)
                    .col(Messages::ChatId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_messages_created_at")
                    .table(Messages::Table)
                    .col(Messages::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // ── memory_entries ─────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(MemoryEntries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MemoryEntries::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MemoryEntries::ScopeType).string().not_null())
                    .col(ColumnDef::new(MemoryEntries::ScopeId).string().not_null())
                    .col(ColumnDef::new(MemoryEntries::Content).text().not_null())
                    .col(ColumnDef::new(MemoryEntries::Embedding).blob().not_null())
                    .col(ColumnDef::new(MemoryEntries::SourceType).string())
                    .col(ColumnDef::new(MemoryEntries::SourceId).string())
                    .col(
                        ColumnDef::new(MemoryEntries::Metadata)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .col(ColumnDef::new(MemoryEntries::CreatedAt).string().not_null())
                    .col(ColumnDef::new(MemoryEntries::ExpiresAt).string())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_memory_scope")
                    .table(MemoryEntries::Table)
                    .col(MemoryEntries::ScopeType)
                    .col(MemoryEntries::ScopeId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_memory_expires")
                    .table(MemoryEntries::Table)
                    .col(MemoryEntries::ExpiresAt)
                    .to_owned(),
            )
            .await?;

        // ── jobs ───────────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Jobs::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Jobs::Id).string().not_null().primary_key())
                    .col(ColumnDef::new(Jobs::Name).string().not_null())
                    .col(ColumnDef::new(Jobs::Description).string())
                    .col(ColumnDef::new(Jobs::JobType).string().not_null())
                    .col(ColumnDef::new(Jobs::CronExpression).string())
                    .col(
                        ColumnDef::new(Jobs::Payload)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(Jobs::Status)
                            .string()
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(Jobs::LastRunAt).string())
                    .col(ColumnDef::new(Jobs::NextRunAt).string())
                    .col(ColumnDef::new(Jobs::LastError).string())
                    .col(
                        ColumnDef::new(Jobs::RetryCount)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Jobs::RetryLimit)
                            .big_integer()
                            .not_null()
                            .default(3),
                    )
                    .col(
                        ColumnDef::new(Jobs::Enabled)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(ColumnDef::new(Jobs::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Jobs::UpdatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_jobs_enabled")
                    .table(Jobs::Table)
                    .col(Jobs::Enabled)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_jobs_next_run_at")
                    .table(Jobs::Table)
                    .col(Jobs::NextRunAt)
                    .to_owned(),
            )
            .await?;

        // ── job_runs ───────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(JobRuns::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(JobRuns::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(JobRuns::JobId).string().not_null())
                    .col(ColumnDef::new(JobRuns::Status).string().not_null())
                    .col(ColumnDef::new(JobRuns::Output).text())
                    .col(ColumnDef::new(JobRuns::Error).text())
                    .col(ColumnDef::new(JobRuns::StartedAt).string().not_null())
                    .col(ColumnDef::new(JobRuns::FinishedAt).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from(JobRuns::Table, JobRuns::JobId)
                            .to(Jobs::Table, Jobs::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_job_runs_job_id")
                    .table(JobRuns::Table)
                    .col(JobRuns::JobId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_job_runs_started_at")
                    .table(JobRuns::Table)
                    .col(JobRuns::StartedAt)
                    .to_owned(),
            )
            .await?;

        // ── subagents ──────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Subagents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Subagents::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Subagents::ParentAgentId).string())
                    .col(
                        ColumnDef::new(Subagents::Depth)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Subagents::Goal).text().not_null())
                    .col(
                        ColumnDef::new(Subagents::Status)
                            .string()
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(Subagents::Provider).string())
                    .col(ColumnDef::new(Subagents::Model).string())
                    .col(
                        ColumnDef::new(Subagents::TokenBudget)
                            .integer()
                            .not_null()
                            .default(8192),
                    )
                    .col(
                        ColumnDef::new(Subagents::TokensUsed)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Subagents::Result).text())
                    .col(ColumnDef::new(Subagents::Error).text())
                    .col(ColumnDef::new(Subagents::ChatId).string())
                    .col(ColumnDef::new(Subagents::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Subagents::UpdatedAt).string().not_null())
                    .col(ColumnDef::new(Subagents::FinishedAt).string())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_subagents_parent")
                    .table(Subagents::Table)
                    .col(Subagents::ParentAgentId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_subagents_status")
                    .table(Subagents::Table)
                    .col(Subagents::Status)
                    .to_owned(),
            )
            .await?;

        // ── tool_invocations ───────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(ToolInvocations::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ToolInvocations::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ToolInvocations::ToolName)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ToolInvocations::ToolCallId).string())
                    .col(ColumnDef::new(ToolInvocations::UserId).string())
                    .col(ColumnDef::new(ToolInvocations::ChatId).string())
                    .col(ColumnDef::new(ToolInvocations::AgentId).string())
                    .col(
                        ColumnDef::new(ToolInvocations::Args)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .col(ColumnDef::new(ToolInvocations::Result).text())
                    .col(
                        ColumnDef::new(ToolInvocations::IsError)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(ToolInvocations::DurationMs).big_integer())
                    .col(ColumnDef::new(ToolInvocations::ApprovedBy).string())
                    .col(
                        ColumnDef::new(ToolInvocations::CreatedAt)
                            .string()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_tool_invocations_tool_name")
                    .table(ToolInvocations::Table)
                    .col(ToolInvocations::ToolName)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_tool_invocations_user_id")
                    .table(ToolInvocations::Table)
                    .col(ToolInvocations::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_tool_invocations_created_at")
                    .table(ToolInvocations::Table)
                    .col(ToolInvocations::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // ── audit_log (append-only) ────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuditLog::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AuditLog::EventType).string().not_null())
                    .col(ColumnDef::new(AuditLog::UserId).string())
                    .col(ColumnDef::new(AuditLog::ChatId).string())
                    .col(ColumnDef::new(AuditLog::AgentId).string())
                    .col(ColumnDef::new(AuditLog::Subject).string().not_null())
                    .col(ColumnDef::new(AuditLog::Action).string().not_null())
                    .col(ColumnDef::new(AuditLog::Outcome).string().not_null())
                    .col(
                        ColumnDef::new(AuditLog::Details)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .col(ColumnDef::new(AuditLog::CreatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_log_event_type")
                    .table(AuditLog::Table)
                    .col(AuditLog::EventType)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_log_user_id")
                    .table(AuditLog::Table)
                    .col(AuditLog::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_audit_log_created_at")
                    .table(AuditLog::Table)
                    .col(AuditLog::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // ── webhooks ───────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Webhooks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Webhooks::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Webhooks::Name).string().not_null())
                    .col(ColumnDef::new(Webhooks::Description).string())
                    .col(ColumnDef::new(Webhooks::Endpoint).string().not_null())
                    .col(ColumnDef::new(Webhooks::Secret).string())
                    .col(
                        ColumnDef::new(Webhooks::Enabled)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(ColumnDef::new(Webhooks::JobId).string())
                    .col(ColumnDef::new(Webhooks::CreatedAt).string().not_null())
                    .col(ColumnDef::new(Webhooks::UpdatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        // ── plugin_state ───────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(PluginState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PluginState::PluginId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PluginState::Version).string().not_null())
                    .col(
                        ColumnDef::new(PluginState::Enabled)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(PluginState::Config)
                            .string()
                            .not_null()
                            .default("{}"),
                    )
                    .col(ColumnDef::new(PluginState::InstalledAt).string().not_null())
                    .col(ColumnDef::new(PluginState::UpdatedAt).string().not_null())
                    .to_owned(),
            )
            .await?;

        // ── config_metadata ────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(ConfigMetadata::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConfigMetadata::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConfigMetadata::Key)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(ConfigMetadata::Value).text().not_null())
                    .col(
                        ColumnDef::new(ConfigMetadata::UpdatedAt)
                            .string()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop tables in reverse dependency order.
        for table in [
            "config_metadata",
            "plugin_state",
            "webhooks",
            "audit_log",
            "tool_invocations",
            "subagents",
            "job_runs",
            "jobs",
            "memory_entries",
            "messages",
            "chats",
            "users",
        ] {
            manager
                .drop_table(Table::drop().table(Alias::new(table)).to_owned())
                .await?;
        }
        Ok(())
    }
}
