use sea_orm::entity::prelude::*;

/// SeaORM entity for the `subagents` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "subagents")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub parent_agent_id: Option<String>,
    pub depth: i32,
    pub goal: String,
    pub status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub token_budget: i32,
    pub tokens_used: i32,
    pub result: Option<String>,
    pub error: Option<String>,
    pub chat_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
