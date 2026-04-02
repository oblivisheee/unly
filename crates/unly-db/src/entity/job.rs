use sea_orm::entity::prelude::*;

/// SeaORM entity for the `jobs` table.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "jobs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// `cron`, `webhook`, `adhoc`, or `health_check`.
    pub job_type: String,
    pub cron_expression: Option<String>,
    /// JSON-encoded payload.
    pub payload: String,
    pub status: String,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub last_error: Option<String>,
    pub retry_count: i64,
    pub retry_limit: i64,
    /// 0 = disabled, 1 = enabled.
    pub enabled: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::job_run::Entity")]
    JobRun,
}

impl Related<super::job_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::JobRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
