use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::DbResult;

/// A row in the audit_log table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRow {
    pub id: String,
    pub event_type: String,
    pub user_id: Option<String>,
    pub chat_id: Option<String>,
    pub agent_id: Option<String>,
    pub subject: String,
    pub action: String,
    pub outcome: String,
    pub details: String,
    pub created_at: DateTime<Utc>,
}

pub struct AuditRepo<'a> {
    pool: &'a SqlitePool,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_audit(row: &sqlx::sqlite::SqliteRow) -> AuditRow {
    let created_at: String = row.try_get("created_at").unwrap_or_default();
    AuditRow {
        id: row.try_get("id").unwrap_or_default(),
        event_type: row.try_get("event_type").unwrap_or_default(),
        user_id: row.try_get("user_id").ok(),
        chat_id: row.try_get("chat_id").ok(),
        agent_id: row.try_get("agent_id").ok(),
        subject: row.try_get("subject").unwrap_or_default(),
        action: row.try_get("action").unwrap_or_default(),
        outcome: row.try_get("outcome").unwrap_or_default(),
        details: row.try_get("details").unwrap_or_else(|_| "{}".to_string()),
        created_at: parse_dt(&created_at),
    }
}

impl<'a> AuditRepo<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, row: &AuditRow) -> DbResult<()> {
        let created_at = row.created_at.to_rfc3339();
        sqlx::query(
            "INSERT INTO audit_log (id, event_type, user_id, chat_id, agent_id, subject, action, outcome, details, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.event_type)
        .bind(&row.user_id)
        .bind(&row.chat_id)
        .bind(&row.agent_id)
        .bind(&row.subject)
        .bind(&row.action)
        .bind(&row.outcome)
        .bind(&row.details)
        .bind(&created_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: i64) -> DbResult<Vec<AuditRow>> {
        let rows = sqlx::query(
            "SELECT id, event_type, user_id, chat_id, agent_id, subject, action, outcome, details, created_at FROM audit_log ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.iter().map(row_to_audit).collect())
    }

    pub async fn list_by_user(&self, user_id: &str, limit: i64) -> DbResult<Vec<AuditRow>> {
        let rows = sqlx::query(
            "SELECT id, event_type, user_id, chat_id, agent_id, subject, action, outcome, details, created_at FROM audit_log WHERE user_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.iter().map(row_to_audit).collect())
    }
}
