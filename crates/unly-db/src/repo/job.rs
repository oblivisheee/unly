use chrono::{DateTime, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::{Deserialize, Serialize};

use crate::{
    entity::{job, job_run},
    error::DbResult,
};

/// Public job row returned from the repository layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub job_type: String,
    pub cron_expression: Option<String>,
    pub payload: String,
    pub status: String,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub retry_count: i64,
    pub retry_limit: i64,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Public job-run row returned from the repository layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRunRow {
    pub id: String,
    pub job_id: String,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_opt_dt(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.map(parse_dt)
}

fn model_to_job(m: job::Model) -> JobRow {
    JobRow {
        id: m.id,
        name: m.name,
        description: m.description,
        job_type: m.job_type,
        cron_expression: m.cron_expression,
        payload: m.payload,
        status: m.status,
        last_run_at: parse_opt_dt(m.last_run_at.as_deref()),
        next_run_at: parse_opt_dt(m.next_run_at.as_deref()),
        last_error: m.last_error,
        retry_count: m.retry_count,
        retry_limit: m.retry_limit,
        enabled: m.enabled != 0,
        created_at: parse_dt(&m.created_at),
        updated_at: parse_dt(&m.updated_at),
    }
}

pub struct JobRepo<'a> {
    conn: &'a DatabaseConnection,
}

impl<'a> JobRepo<'a> {
    pub fn new(conn: &'a DatabaseConnection) -> Self {
        Self { conn }
    }

    pub async fn upsert(&self, row: &JobRow) -> DbResult<()> {
        let active = job::ActiveModel {
            id: Set(row.id.clone()),
            name: Set(row.name.clone()),
            description: Set(row.description.clone()),
            job_type: Set(row.job_type.clone()),
            cron_expression: Set(row.cron_expression.clone()),
            payload: Set(row.payload.clone()),
            status: Set(row.status.clone()),
            last_run_at: Set(row.last_run_at.map(|t| t.to_rfc3339())),
            next_run_at: Set(row.next_run_at.map(|t| t.to_rfc3339())),
            last_error: Set(row.last_error.clone()),
            retry_count: Set(row.retry_count),
            retry_limit: Set(row.retry_limit),
            enabled: Set(if row.enabled { 1 } else { 0 }),
            created_at: Set(row.created_at.to_rfc3339()),
            updated_at: Set(row.updated_at.to_rfc3339()),
        };
        job::Entity::insert(active)
            .on_conflict(
                sea_orm::sea_query::OnConflict::column(job::Column::Id)
                    .update_columns([
                        job::Column::Name,
                        job::Column::Description,
                        job::Column::JobType,
                        job::Column::CronExpression,
                        job::Column::Payload,
                        job::Column::Status,
                        job::Column::LastRunAt,
                        job::Column::NextRunAt,
                        job::Column::LastError,
                        job::Column::RetryCount,
                        job::Column::RetryLimit,
                        job::Column::Enabled,
                        job::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(self.conn)
            .await?;
        Ok(())
    }

    pub async fn list_enabled(&self) -> DbResult<Vec<JobRow>> {
        let models = job::Entity::find()
            .filter(job::Column::Enabled.eq(1))
            .order_by_asc(job::Column::Name)
            .all(self.conn)
            .await?;
        Ok(models.into_iter().map(model_to_job).collect())
    }

    pub async fn find_by_id(&self, id: &str) -> DbResult<Option<JobRow>> {
        let model = job::Entity::find_by_id(id).one(self.conn).await?;
        Ok(model.map(model_to_job))
    }

    pub async fn insert_run(&self, row: &JobRunRow) -> DbResult<()> {
        let active = job_run::ActiveModel {
            id: Set(row.id.clone()),
            job_id: Set(row.job_id.clone()),
            status: Set(row.status.clone()),
            output: Set(row.output.clone()),
            error: Set(row.error.clone()),
            started_at: Set(row.started_at.to_rfc3339()),
            finished_at: Set(row.finished_at.map(|t| t.to_rfc3339())),
        };
        job_run::Entity::insert(active).exec(self.conn).await?;
        Ok(())
    }
}
