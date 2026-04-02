use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    entity::{chat, message},
    error::DbResult,
};

/// Public chat row returned from the repository layer.
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

/// Public message row returned from the repository layer.
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

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn model_to_chat(m: chat::Model) -> ChatRow {
    ChatRow {
        id: m.id,
        telegram_chat_id: m.telegram_chat_id,
        title: m.title,
        system_prompt: m.system_prompt,
        provider: m.provider,
        model: m.model,
        created_at: parse_dt(&m.created_at),
        updated_at: parse_dt(&m.updated_at),
        metadata: m.metadata,
    }
}

fn model_to_message(m: message::Model) -> MessageRow {
    MessageRow {
        id: m.id,
        chat_id: m.chat_id,
        user_id: m.user_id,
        role: m.role,
        content: m.content,
        created_at: parse_dt(&m.created_at),
        metadata: m.metadata,
    }
}

pub struct ChatRepo<'a> {
    conn: &'a DatabaseConnection,
}

impl<'a> ChatRepo<'a> {
    pub fn new(conn: &'a DatabaseConnection) -> Self {
        Self { conn }
    }

    pub async fn upsert(&self, row: &ChatRow) -> DbResult<()> {
        let active = chat::ActiveModel {
            id: Set(row.id.clone()),
            telegram_chat_id: Set(row.telegram_chat_id),
            title: Set(row.title.clone()),
            system_prompt: Set(row.system_prompt.clone()),
            provider: Set(row.provider.clone()),
            model: Set(row.model.clone()),
            created_at: Set(row.created_at.to_rfc3339()),
            updated_at: Set(row.updated_at.to_rfc3339()),
            metadata: Set(row.metadata.clone()),
        };
        chat::Entity::insert(active)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(chat::Column::Id)
                    .update_columns([
                        chat::Column::TelegramChatId,
                        chat::Column::Title,
                        chat::Column::SystemPrompt,
                        chat::Column::Provider,
                        chat::Column::Model,
                        chat::Column::UpdatedAt,
                        chat::Column::Metadata,
                    ])
                    .to_owned(),
            )
            .exec(self.conn)
            .await?;
        Ok(())
    }

    pub async fn find_by_telegram_id(&self, telegram_chat_id: i64) -> DbResult<Option<ChatRow>> {
        let model = chat::Entity::find()
            .filter(chat::Column::TelegramChatId.eq(telegram_chat_id))
            .one(self.conn)
            .await?;
        Ok(model.map(model_to_chat))
    }

    pub async fn find_by_id(&self, id: &str) -> DbResult<Option<ChatRow>> {
        let model = chat::Entity::find_by_id(id).one(self.conn).await?;
        Ok(model.map(model_to_chat))
    }

    pub async fn list_messages(&self, chat_id: &str, limit: u64) -> DbResult<Vec<MessageRow>> {
        // Fetch the N most recent messages, then reverse for chronological order.
        let models = message::Entity::find()
            .filter(message::Column::ChatId.eq(chat_id))
            .order_by_desc(message::Column::CreatedAt)
            .limit(limit)
            .all(self.conn)
            .await?;
        let mut rows: Vec<MessageRow> = models.into_iter().map(model_to_message).collect();
        rows.reverse();
        Ok(rows)
    }

    pub async fn insert_message(&self, row: &MessageRow) -> DbResult<()> {
        let active = message::ActiveModel {
            id: Set(row.id.clone()),
            chat_id: Set(row.chat_id.clone()),
            user_id: Set(row.user_id.clone()),
            role: Set(row.role.clone()),
            content: Set(row.content.clone()),
            created_at: Set(row.created_at.to_rfc3339()),
            metadata: Set(row.metadata.clone()),
        };
        message::Entity::insert(active).exec(self.conn).await?;
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
        let now = Utc::now();
        let row = ChatRow {
            id: Uuid::new_v4().to_string(),
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
