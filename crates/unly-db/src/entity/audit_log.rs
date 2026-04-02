use sea_orm::entity::prelude::*;

/// SeaORM entity for the `audit_log` table (append-only).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub event_type: String,
    pub user_id: Option<String>,
    pub chat_id: Option<String>,
    pub agent_id: Option<String>,
    pub subject: String,
    pub action: String,
    /// `success`, `failure`, or `denied`.
    pub outcome: String,
    /// JSON object with additional details.
    pub details: String,
    /// ISO 8601 timestamp stored as TEXT.
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
