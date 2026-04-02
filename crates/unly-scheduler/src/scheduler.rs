use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use cron::Schedule;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use unly_db::{
    repo::job::{JobRow, JobRunRow},
    Database,
};

use crate::{
    error::SchedulerError,
    job::{JobDefinition, JobType},
};

/// Callback type for job execution.
pub type JobCallback =
    Arc<dyn Fn(serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send>> + Send + Sync>;

/// The scheduler manages cron jobs and dispatches them at the right time.
pub struct Scheduler {
    db: Database,
    jobs: Arc<RwLock<HashMap<String, (JobDefinition, JobCallback)>>>,
    max_concurrent: usize,
}

impl Scheduler {
    pub fn new(db: Database, max_concurrent: usize) -> Self {
        Self {
            db,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent,
        }
    }

    /// Register a job definition with a callback.
    pub async fn register(&self, job: JobDefinition, callback: JobCallback) {
        info!("registering job: {} ({})", job.name, job.id);
        self.jobs.write().await.insert(job.id.clone(), (job.clone(), callback));

        // Persist to database.
        let now = Utc::now();
        let row = JobRow {
            id: job.id,
            name: job.name,
            description: job.description,
            job_type: job.job_type.to_string(),
            cron_expression: job.cron_expression,
            payload: serde_json::to_string(&job.payload).unwrap_or_default(),
            status: "pending".to_string(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
            retry_count: 0,
            retry_limit: job.retry_limit,
            enabled: job.enabled,
            created_at: now,
            updated_at: now,
        };

        let repo = unly_db::repo::job::JobRepo::new(self.db.pool());
        if let Err(e) = repo.upsert(&row).await {
            warn!("failed to persist job definition: {}", e);
        }
    }

    /// Start the scheduler loop (runs indefinitely, should be spawned as a task).
    pub async fn run(self: Arc<Self>) {
        info!("scheduler started");
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let now = Utc::now();
            debug!("scheduler tick at {}", now);

            let jobs = self.jobs.read().await;
            let semaphore = Arc::new(tokio::sync::Semaphore::new(self.max_concurrent));

            for (id, (job_def, callback)) in jobs.iter() {
                if !job_def.enabled {
                    continue;
                }

                // Check if the cron schedule fires now.
                if let Some(cron_expr) = &job_def.cron_expression {
                    let should_run = match Schedule::from_str(cron_expr) {
                        Ok(schedule) => {
                            let upcoming = schedule.upcoming(Utc).next();
                            upcoming.map(|next| (next - now).num_seconds().abs() < 60).unwrap_or(false)
                        }
                        Err(e) => {
                            warn!("invalid cron expression for job {}: {}", id, e);
                            false
                        }
                    };

                    if should_run {
                        let permit = semaphore.clone().acquire_owned().await;
                        let callback = callback.clone();
                        let payload = job_def.payload.clone();
                        let job_id = id.clone();
                        let db = self.db.clone();

                        tokio::spawn(async move {
                            let _permit = permit;
                            let run_id = Uuid::new_v4().to_string();
                            let started_at = Utc::now();
                            info!("executing job: {}", job_id);

                            let result = callback(payload).await;
                            let finished_at = Utc::now();

                            let (status, output, error) = match &result {
                                Ok(out) => ("completed".to_string(), Some(out.clone()), None),
                                Err(err) => ("failed".to_string(), None, Some(err.clone())),
                            };

                            if let Err(e) = result {
                                error!("job {} failed: {}", job_id, e);
                            }

                            let run_row = JobRunRow {
                                id: run_id,
                                job_id,
                                status,
                                output,
                                error,
                                started_at,
                                finished_at: Some(finished_at),
                            };

                            let repo = unly_db::repo::job::JobRepo::new(db.pool());
                            if let Err(e) = repo.insert_run(&run_row).await {
                                warn!("failed to persist job run: {}", e);
                            }
                        });
                    }
                }
            }
        }
    }
}
