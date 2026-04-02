use sea_orm::entity::prelude::*;

/// SeaORM entity for the `messages` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "messages")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub chat_id: String,
    pub user_id: Option<String>,
    pub role: String,
    /// JSON-encoded message content.
    pub content: String,
    /// ISO 8601 timestamp stored as TEXT.
    pub created_at: String,
    /// JSON object (default `{}`).
    pub metadata: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chat::Entity",
        from = "Column::ChatId",
        to = "super::chat::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Chat,
}

impl Related<super::chat::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Chat.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
