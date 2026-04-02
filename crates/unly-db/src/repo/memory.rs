use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};

use crate::{entity::memory_entry, error::DbResult};

/// Public memory-entry row returned from the repository layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntryRow {
    pub id: String,
    pub scope_type: String,
    pub scope_id: String,
    pub content: String,
    /// Serialized embedding vector (little-endian f32 sequence).
    pub embedding: Vec<u8>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub metadata: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn model_to_entry(m: memory_entry::Model) -> MemoryEntryRow {
    MemoryEntryRow {
        id: m.id,
        scope_type: m.scope_type,
        scope_id: m.scope_id,
        content: m.content,
        embedding: m.embedding,
        source_type: m.source_type,
        source_id: m.source_id,
        metadata: m.metadata,
        created_at: parse_dt(&m.created_at),
        expires_at: m.expires_at.as_deref().map(parse_dt),
    }
}

pub struct MemoryRepo<'a> {
    conn: &'a DatabaseConnection,
}

impl<'a> MemoryRepo<'a> {
    pub fn new(conn: &'a DatabaseConnection) -> Self {
        Self { conn }
    }

    pub async fn insert(&self, row: &MemoryEntryRow) -> DbResult<()> {
        let active = memory_entry::ActiveModel {
            id: Set(row.id.clone()),
            scope_type: Set(row.scope_type.clone()),
            scope_id: Set(row.scope_id.clone()),
            content: Set(row.content.clone()),
            embedding: Set(row.embedding.clone()),
            source_type: Set(row.source_type.clone()),
            source_id: Set(row.source_id.clone()),
            metadata: Set(row.metadata.clone()),
            created_at: Set(row.created_at.to_rfc3339()),
            expires_at: Set(row.expires_at.map(|t| t.to_rfc3339())),
        };
        memory_entry::Entity::insert(active)
            .exec(self.conn)
            .await?;
        Ok(())
    }

    /// Return all non-expired entries for a given scope, ordered newest first.
    pub async fn list_by_scope(
        &self,
        scope_type: &str,
        scope_id: &str,
    ) -> DbResult<Vec<MemoryEntryRow>> {
        let now = Utc::now().to_rfc3339();
        let models = memory_entry::Entity::find()
            .filter(memory_entry::Column::ScopeType.eq(scope_type))
            .filter(memory_entry::Column::ScopeId.eq(scope_id))
            .filter(
                sea_orm::Condition::any()
                    .add(memory_entry::Column::ExpiresAt.is_null())
                    .add(memory_entry::Column::ExpiresAt.gt(now)),
            )
            .order_by_desc(memory_entry::Column::CreatedAt)
            .all(self.conn)
            .await?;
        Ok(models.into_iter().map(model_to_entry).collect())
    }

    /// Delete entries whose `expires_at` is in the past; returns count removed.
    pub async fn delete_expired(&self) -> DbResult<u64> {
        let now = Utc::now().to_rfc3339();
        let result = memory_entry::Entity::delete_many()
            .filter(memory_entry::Column::ExpiresAt.is_not_null())
            .filter(memory_entry::Column::ExpiresAt.lte(now))
            .exec(self.conn)
            .await?;
        Ok(result.rows_affected)
    }

    pub async fn delete_by_id(&self, id: &str) -> DbResult<bool> {
        let result = memory_entry::Entity::delete_by_id(id)
            .exec(self.conn)
            .await?;
        Ok(result.rows_affected > 0)
    }

    pub async fn count_by_scope(&self, scope_type: &str, scope_id: &str) -> DbResult<u64> {
        use sea_orm::PaginatorTrait;
        let count = memory_entry::Entity::find()
            .filter(memory_entry::Column::ScopeType.eq(scope_type))
            .filter(memory_entry::Column::ScopeId.eq(scope_id))
            .count(self.conn)
            .await?;
        Ok(count)
    }
}
