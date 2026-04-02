use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::{Deserialize, Serialize};

use crate::{entity::user, error::DbResult};

/// Public user row returned from the repository layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRow {
    pub id: String,
    pub telegram_user_id: Option<i64>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub role: String,
    pub permissions: String,
    pub is_blocked: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn model_to_user(m: user::Model) -> UserRow {
    UserRow {
        id: m.id,
        telegram_user_id: m.telegram_user_id,
        username: m.username,
        display_name: m.display_name,
        role: m.role,
        permissions: m.permissions,
        is_blocked: m.is_blocked != 0,
        created_at: parse_dt(&m.created_at),
        updated_at: parse_dt(&m.updated_at),
    }
}

pub struct UserRepo<'a> {
    conn: &'a DatabaseConnection,
}

impl<'a> UserRepo<'a> {
    pub fn new(conn: &'a DatabaseConnection) -> Self {
        Self { conn }
    }

    pub async fn upsert(&self, row: &UserRow) -> DbResult<()> {
        let active = user::ActiveModel {
            id: Set(row.id.clone()),
            telegram_user_id: Set(row.telegram_user_id),
            username: Set(row.username.clone()),
            display_name: Set(row.display_name.clone()),
            role: Set(row.role.clone()),
            permissions: Set(row.permissions.clone()),
            is_blocked: Set(if row.is_blocked { 1 } else { 0 }),
            created_at: Set(row.created_at.to_rfc3339()),
            updated_at: Set(row.updated_at.to_rfc3339()),
        };
        user::Entity::insert(active)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(user::Column::Id)
                    .update_columns([
                        user::Column::TelegramUserId,
                        user::Column::Username,
                        user::Column::DisplayName,
                        user::Column::Role,
                        user::Column::Permissions,
                        user::Column::IsBlocked,
                        user::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(self.conn)
            .await?;
        Ok(())
    }

    pub async fn find_by_telegram_id(&self, telegram_user_id: i64) -> DbResult<Option<UserRow>> {
        let model = user::Entity::find()
            .filter(user::Column::TelegramUserId.eq(telegram_user_id))
            .one(self.conn)
            .await?;
        Ok(model.map(model_to_user))
    }

    pub async fn find_by_id(&self, id: &str) -> DbResult<Option<UserRow>> {
        let model = user::Entity::find_by_id(id).one(self.conn).await?;
        Ok(model.map(model_to_user))
    }

    pub async fn list(&self, limit: u64) -> DbResult<Vec<UserRow>> {
        let models = user::Entity::find()
            .order_by_desc(user::Column::CreatedAt)
            .limit(limit)
            .all(self.conn)
            .await?;
        Ok(models.into_iter().map(model_to_user).collect())
    }
}
