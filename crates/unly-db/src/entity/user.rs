use sea_orm::entity::prelude::*;

/// SeaORM entity for the `users` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub telegram_user_id: Option<i64>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    /// JSON-encoded permission set.
    pub permissions: String,
    /// 0 = not blocked, 1 = blocked (SQLite stores booleans as integers).
    pub is_blocked: i32,
    /// ISO 8601 timestamp stored as TEXT.
    pub created_at: String,
    /// ISO 8601 timestamp stored as TEXT.
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
