use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::error::DbResult;

/// A row in the jobs table.
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

/// A row in the job_runs table.
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
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_job(row: &sqlx::sqlite::SqliteRow) -> JobRow {
    let created_at: String = row.try_get("created_at").unwrap_or_default();
    let updated_at: String = row.try_get("updated_at").unwrap_or_default();
    let last_run_at: Option<String> = row.try_get("last_run_at").ok();
    let next_run_at: Option<String> = row.try_get("next_run_at").ok();
    let enabled: i64 = row.try_get("enabled").unwrap_or(1);
    JobRow {
        id: row.try_get("id").unwrap_or_default(),
        name: row.try_get("name").unwrap_or_default(),
        description: row.try_get("description").ok(),
        job_type: row.try_get("job_type").unwrap_or_default(),
        cron_expression: row.try_get("cron_expression").ok(),
        payload: row.try_get("payload").unwrap_or_else(|_| "{}".to_string()),
        status: row.try_get("status").unwrap_or_else(|_| "pending".to_string()),
        last_run_at: last_run_at.as_deref().map(parse_dt),
        next_run_at: next_run_at.as_deref().map(parse_dt),
        last_error: row.try_get("last_error").ok(),
        retry_count: row.try_get("retry_count").unwrap_or(0),
        retry_limit: row.try_get("retry_limit").unwrap_or(3),
        enabled: enabled != 0,
        created_at: parse_dt(&created_at),
        updated_at: parse_dt(&updated_at),
    }
}

pub struct JobRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> JobRepo<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert(&self, row: &JobRow) -> DbResult<()> {
        let created_at = row.created_at.to_rfc3339();
        let updated_at = row.updated_at.to_rfc3339();
        let enabled: i64 = if row.enabled { 1 } else { 0 };
        let last_run_at = row.last_run_at.map(|t| t.to_rfc3339());
        let next_run_at = row.next_run_at.map(|t| t.to_rfc3339());
        sqlx::query(
            r#"INSERT INTO jobs (id, name, description, job_type, cron_expression, payload, status, last_run_at, next_run_at, last_error, retry_count, retry_limit, enabled, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                job_type = excluded.job_type,
                cron_expression = excluded.cron_expression,
                payload = excluded.payload,
                status = excluded.status,
                last_run_at = excluded.last_run_at,
                next_run_at = excluded.next_run_at,
                last_error = excluded.last_error,
                retry_count = excluded.retry_count,
                retry_limit = excluded.retry_limit,
                enabled = excluded.enabled,
                updated_at = excluded.updated_at"#,
        )
        .bind(&row.id)
        .bind(&row.name)
        .bind(&row.description)
        .bind(&row.job_type)
        .bind(&row.cron_expression)
        .bind(&row.payload)
        .bind(&row.status)
        .bind(&last_run_at)
        .bind(&next_run_at)
        .bind(&row.last_error)
        .bind(row.retry_count)
        .bind(row.retry_limit)
        .bind(enabled)
        .bind(&created_at)
        .bind(&updated_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_enabled(&self) -> DbResult<Vec<JobRow>> {
        let rows = sqlx::query(
            "SELECT id, name, description, job_type, cron_expression, payload, status, last_run_at, next_run_at, last_error, retry_count, retry_limit, enabled, created_at, updated_at FROM jobs WHERE enabled = 1 ORDER BY name",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(rows.iter().map(row_to_job).collect())
    }

    pub async fn find_by_id(&self, id: &str) -> DbResult<Option<JobRow>> {
        let row = sqlx::query(
            "SELECT id, name, description, job_type, cron_expression, payload, status, last_run_at, next_run_at, last_error, retry_count, retry_limit, enabled, created_at, updated_at FROM jobs WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.as_ref().map(row_to_job))
    }

    pub async fn insert_run(&self, row: &JobRunRow) -> DbResult<()> {
        let started_at = row.started_at.to_rfc3339();
        let finished_at = row.finished_at.map(|t| t.to_rfc3339());
        sqlx::query(
            "INSERT INTO job_runs (id, job_id, status, output, error, started_at, finished_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.job_id)
        .bind(&row.status)
        .bind(&row.output)
        .bind(&row.error)
        .bind(&started_at)
        .bind(&finished_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }
}
