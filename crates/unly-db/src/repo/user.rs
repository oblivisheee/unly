use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::DbResult;

/// A row in the users table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRow {
    pub id: String,
    pub telegram_user_id: Option<i64>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    pub permissions: String,
    pub is_blocked: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct UserRepo<'a> {
    pool: &'a SqlitePool,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> UserRow {
    let created_at: String = row.try_get("created_at").unwrap_or_default();
    let updated_at: String = row.try_get("updated_at").unwrap_or_default();
    let is_blocked: i64 = row.try_get("is_blocked").unwrap_or(0);
    UserRow {
        id: row.try_get("id").unwrap_or_default(),
        telegram_user_id: row.try_get("telegram_user_id").ok(),
        username: row.try_get("username").ok(),
        display_name: row.try_get("display_name").ok(),
        role: row.try_get("role").unwrap_or_else(|_| "user".to_string()),
        permissions: row.try_get("permissions").unwrap_or_else(|_| "{}".to_string()),
        is_blocked: is_blocked != 0,
        created_at: parse_dt(&created_at),
        updated_at: parse_dt(&updated_at),
    }
}

impl<'a> UserRepo<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert(&self, row: &UserRow) -> DbResult<()> {
        let created_at = row.created_at.to_rfc3339();
        let updated_at = row.updated_at.to_rfc3339();
        let is_blocked: i64 = if row.is_blocked { 1 } else { 0 };
        sqlx::query(
            r#"INSERT INTO users (id, telegram_user_id, username, display_name, role, permissions, is_blocked, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                telegram_user_id = excluded.telegram_user_id,
                username = excluded.username,
                display_name = excluded.display_name,
                role = excluded.role,
                permissions = excluded.permissions,
                is_blocked = excluded.is_blocked,
                updated_at = excluded.updated_at"#,
        )
        .bind(&row.id)
        .bind(row.telegram_user_id)
        .bind(&row.username)
        .bind(&row.display_name)
        .bind(&row.role)
        .bind(&row.permissions)
        .bind(is_blocked)
        .bind(&created_at)
        .bind(&updated_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_telegram_id(&self, telegram_user_id: i64) -> DbResult<Option<UserRow>> {
        let row = sqlx::query(
            "SELECT id, telegram_user_id, username, display_name, role, permissions, is_blocked, created_at, updated_at FROM users WHERE telegram_user_id = ?",
        )
        .bind(telegram_user_id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.as_ref().map(row_to_user))
    }

    pub async fn find_by_id(&self, id: &str) -> DbResult<Option<UserRow>> {
        let row = sqlx::query(
            "SELECT id, telegram_user_id, username, display_name, role, permissions, is_blocked, created_at, updated_at FROM users WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.as_ref().map(row_to_user))
    }

    pub async fn list(&self, limit: i64) -> DbResult<Vec<UserRow>> {
        let rows = sqlx::query(
            "SELECT id, telegram_user_id, username, display_name, role, permissions, is_blocked, created_at, updated_at FROM users ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.iter().map(row_to_user).collect())
    }
}
