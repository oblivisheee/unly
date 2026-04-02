use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::DbResult;

/// A row in the memory_entries table (vector memory store).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntryRow {
    pub id: String,
    pub scope_type: String,
    pub scope_id: String,
    pub content: String,
    pub embedding: Vec<u8>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub metadata: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_memory(row: &sqlx::sqlite::SqliteRow) -> MemoryEntryRow {
    let created_at: String = row.try_get("created_at").unwrap_or_default();
    let expires_at: Option<String> = row.try_get("expires_at").ok();
    MemoryEntryRow {
        id: row.try_get("id").unwrap_or_default(),
        scope_type: row.try_get("scope_type").unwrap_or_default(),
        scope_id: row.try_get("scope_id").unwrap_or_default(),
        content: row.try_get("content").unwrap_or_default(),
        embedding: row.try_get("embedding").unwrap_or_default(),
        source_type: row.try_get("source_type").ok(),
        source_id: row.try_get("source_id").ok(),
        metadata: row.try_get("metadata").unwrap_or_else(|_| "{}".to_string()),
        created_at: parse_dt(&created_at),
        expires_at: expires_at.as_deref().map(parse_dt),
    }
}

pub struct MemoryRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> MemoryRepo<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, row: &MemoryEntryRow) -> DbResult<()> {
        let created_at = row.created_at.to_rfc3339();
        let expires_at = row.expires_at.map(|t| t.to_rfc3339());
        sqlx::query(
            "INSERT INTO memory_entries (id, scope_type, scope_id, content, embedding, source_type, source_id, metadata, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.scope_type)
        .bind(&row.scope_id)
        .bind(&row.content)
        .bind(&row.embedding)
        .bind(&row.source_type)
        .bind(&row.source_id)
        .bind(&row.metadata)
        .bind(&created_at)
        .bind(&expires_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_by_scope(
        &self,
        scope_type: &str,
        scope_id: &str,
    ) -> DbResult<Vec<MemoryEntryRow>> {
        let rows = sqlx::query(
            "SELECT id, scope_type, scope_id, content, embedding, source_type, source_id, metadata, created_at, expires_at FROM memory_entries WHERE scope_type = ? AND scope_id = ? AND (expires_at IS NULL OR expires_at > datetime('now')) ORDER BY created_at DESC",
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.iter().map(row_to_memory).collect())
    }

    pub async fn delete_expired(&self) -> DbResult<u64> {
        let result = sqlx::query(
            "DELETE FROM memory_entries WHERE expires_at IS NOT NULL AND expires_at <= datetime('now')",
        )
        .execute(self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn delete_by_id(&self, id: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM memory_entries WHERE id = ?")
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn count_by_scope(&self, scope_type: &str, scope_id: &str) -> DbResult<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) as count FROM memory_entries WHERE scope_type = ? AND scope_id = ?",
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_one(self.pool)
        .await?;
        Ok(row.try_get("count").unwrap_or(0))
    }
}
