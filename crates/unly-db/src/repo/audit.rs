use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};

use crate::{entity::audit_log, error::DbResult};

/// Public audit row returned from the repository layer.
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

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn model_to_audit(m: audit_log::Model) -> AuditRow {
    AuditRow {
        id: m.id,
        event_type: m.event_type,
        user_id: m.user_id,
        chat_id: m.chat_id,
        agent_id: m.agent_id,
        subject: m.subject,
        action: m.action,
        outcome: m.outcome,
        details: m.details,
        created_at: parse_dt(&m.created_at),
    }
}

pub struct AuditRepo<'a> {
    conn: &'a DatabaseConnection,
}

impl<'a> AuditRepo<'a> {
    pub fn new(conn: &'a DatabaseConnection) -> Self {
        Self { conn }
    }

    pub async fn insert(&self, row: &AuditRow) -> DbResult<()> {
        let active = audit_log::ActiveModel {
            id: Set(row.id.clone()),
            event_type: Set(row.event_type.clone()),
            user_id: Set(row.user_id.clone()),
            chat_id: Set(row.chat_id.clone()),
            agent_id: Set(row.agent_id.clone()),
            subject: Set(row.subject.clone()),
            action: Set(row.action.clone()),
            outcome: Set(row.outcome.clone()),
            details: Set(row.details.clone()),
            created_at: Set(row.created_at.to_rfc3339()),
        };
        audit_log::Entity::insert(active).exec(self.conn).await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: u64) -> DbResult<Vec<AuditRow>> {
        let models = audit_log::Entity::find()
            .order_by_desc(audit_log::Column::CreatedAt)
            .limit(limit)
            .all(self.conn)
            .await?;
        Ok(models.into_iter().map(model_to_audit).collect())
    }

    pub async fn list_by_user(&self, user_id: &str, limit: u64) -> DbResult<Vec<AuditRow>> {
        let models = audit_log::Entity::find()
            .filter(audit_log::Column::UserId.eq(user_id))
            .order_by_desc(audit_log::Column::CreatedAt)
            .limit(limit)
            .all(self.conn)
            .await?;
        Ok(models.into_iter().map(model_to_audit).collect())
    }
}
