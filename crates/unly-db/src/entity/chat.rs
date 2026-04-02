use sea_orm::entity::prelude::*;

/// SeaORM entity for the `chats` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "chats")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub telegram_chat_id: Option<i64>,
    pub title: Option<String>,
    pub system_prompt: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// ISO 8601 timestamp stored as TEXT.
    pub created_at: String,
    /// ISO 8601 timestamp stored as TEXT.
    pub updated_at: String,
    /// JSON object (default `{}`).
    pub metadata: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::message::Entity")]
    Message,
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Message.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
