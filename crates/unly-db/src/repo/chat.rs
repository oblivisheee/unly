use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::DbResult;

/// A row in the chats table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRow {
    pub id: String,
    pub telegram_chat_id: Option<i64>,
    pub title: Option<String>,
    pub system_prompt: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: String,
}

/// A row in the messages table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: String,
    pub chat_id: String,
    pub user_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub metadata: String,
}

pub struct ChatRepo<'a> {
    pool: &'a SqlitePool,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
                .map(|ndt| ndt.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

impl<'a> ChatRepo<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    fn row_to_chat(row: &sqlx::sqlite::SqliteRow) -> ChatRow {
        let created_at: String = row.try_get("created_at").unwrap_or_default();
        let updated_at: String = row.try_get("updated_at").unwrap_or_default();
        ChatRow {
            id: row.try_get("id").unwrap_or_default(),
            telegram_chat_id: row.try_get("telegram_chat_id").ok(),
            title: row.try_get("title").ok(),
            system_prompt: row.try_get("system_prompt").ok(),
            provider: row.try_get("provider").ok(),
            model: row.try_get("model").ok(),
            created_at: parse_dt(&created_at),
            updated_at: parse_dt(&updated_at),
            metadata: row.try_get("metadata").unwrap_or_else(|_| "{}".to_string()),
        }
    }

    fn row_to_message(row: &sqlx::sqlite::SqliteRow) -> MessageRow {
        let created_at: String = row.try_get("created_at").unwrap_or_default();
        MessageRow {
            id: row.try_get("id").unwrap_or_default(),
            chat_id: row.try_get("chat_id").unwrap_or_default(),
            user_id: row.try_get("user_id").ok(),
            role: row.try_get("role").unwrap_or_default(),
            content: row.try_get("content").unwrap_or_default(),
            created_at: parse_dt(&created_at),
            metadata: row.try_get("metadata").unwrap_or_else(|_| "{}".to_string()),
        }
    }

    pub async fn upsert(&self, row: &ChatRow) -> DbResult<()> {
        let created_at = row.created_at.to_rfc3339();
        let updated_at = row.updated_at.to_rfc3339();
        sqlx::query(
            r#"INSERT INTO chats (id, telegram_chat_id, title, system_prompt, provider, model, created_at, updated_at, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                telegram_chat_id = excluded.telegram_chat_id,
                title = excluded.title,
                system_prompt = excluded.system_prompt,
                provider = excluded.provider,
                model = excluded.model,
                updated_at = excluded.updated_at,
                metadata = excluded.metadata"#,
        )
        .bind(&row.id)
        .bind(row.telegram_chat_id)
        .bind(&row.title)
        .bind(&row.system_prompt)
        .bind(&row.provider)
        .bind(&row.model)
        .bind(&created_at)
        .bind(&updated_at)
        .bind(&row.metadata)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_telegram_id(&self, telegram_chat_id: i64) -> DbResult<Option<ChatRow>> {
        let row = sqlx::query(
            "SELECT id, telegram_chat_id, title, system_prompt, provider, model, created_at, updated_at, metadata FROM chats WHERE telegram_chat_id = ?",
        )
        .bind(telegram_chat_id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.as_ref().map(Self::row_to_chat))
    }

    pub async fn find_by_id(&self, id: &str) -> DbResult<Option<ChatRow>> {
        let row = sqlx::query(
            "SELECT id, telegram_chat_id, title, system_prompt, provider, model, created_at, updated_at, metadata FROM chats WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.as_ref().map(Self::row_to_chat))
    }

    pub async fn list_messages(&self, chat_id: &str, limit: i64) -> DbResult<Vec<MessageRow>> {
        let rows = sqlx::query(
            "SELECT id, chat_id, user_id, role, content, created_at, metadata FROM messages WHERE chat_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(chat_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        let mut result: Vec<MessageRow> = rows.iter().map(Self::row_to_message).collect();
        result.reverse();
        Ok(result)
    }

    pub async fn insert_message(&self, row: &MessageRow) -> DbResult<()> {
        let created_at = row.created_at.to_rfc3339();
        sqlx::query(
            "INSERT INTO messages (id, chat_id, user_id, role, content, created_at, metadata) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.chat_id)
        .bind(&row.user_id)
        .bind(&row.role)
        .bind(&row.content)
        .bind(&created_at)
        .bind(&row.metadata)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_or_create_chat(
        &self,
        telegram_chat_id: i64,
        title: Option<&str>,
    ) -> DbResult<ChatRow> {
        if let Some(row) = self.find_by_telegram_id(telegram_chat_id).await? {
            return Ok(row);
        }
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let row = ChatRow {
            id,
            telegram_chat_id: Some(telegram_chat_id),
            title: title.map(|t| t.to_string()),
            system_prompt: None,
            provider: None,
            model: None,
            created_at: now,
            updated_at: now,
            metadata: "{}".to_string(),
        };
        self.upsert(&row).await?;
        Ok(row)
    }
}
