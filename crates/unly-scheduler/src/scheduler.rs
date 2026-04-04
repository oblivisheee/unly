use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use unly_db::{
    repo::job::{JobRow, JobRunRow},
    Database,
};

use crate::job::JobDefinition;

/// Callback type for job execution.
pub type JobCallback = Arc<
    dyn Fn(
            serde_json::Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send>,
        > + Send
        + Sync,
>;

/// The scheduler manages cron jobs and dispatches them at the right time.
pub struct Scheduler {
    db: Database,
    jobs: Arc<RwLock<HashMap<String, (JobDefinition, JobCallback)>>>,
    /// Tracks the last time each job was dispatched to prevent duplicate firing
    /// within the same scheduling window.
    last_dispatched: Arc<RwLock<HashMap<String, DateTime<Utc>>>>,
    max_concurrent: usize,
}

impl Scheduler {
    pub fn new(db: Database, max_concurrent: usize) -> Self {
        Self {
            db,
            jobs: Arc::new(RwLock::new(HashMap::new())),
            last_dispatched: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent,
        }
    }

    /// Register a job definition with a callback.
    pub async fn register(&self, job: JobDefinition, callback: JobCallback) {
        info!("registering job: {} ({})", job.name, job.id);
        self.jobs
            .write()
            .await
            .insert(job.id.clone(), (job.clone(), callback));

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

        let repo = unly_db::repo::job::JobRepo::new(self.db.conn());
        if let Err(e) = repo.upsert(&row).await {
            warn!("failed to persist job definition: {}", e);
        }
    }

    /// Register a job in memory only (no database upsert).
    ///
    /// Used when restoring persisted jobs on startup — the definition is
    /// already in the DB so we only need to (re-)add it to the dispatch map.
    pub async fn register_in_memory(&self, job: JobDefinition, callback: JobCallback) {
        info!("restoring job from db: {} ({})", job.name, job.id);
        self.jobs
            .write()
            .await
            .insert(job.id.clone(), (job, callback));
    }

    pub async fn set_job_enabled(&self, id: &str, enabled: bool) -> bool {
        let found = {
            let mut jobs = self.jobs.write().await;
            if let Some((job, _)) = jobs.get_mut(id) {
                job.enabled = enabled;
                true
            } else {
                false
            }
        };
        if !enabled {
            self.last_dispatched.write().await.remove(id);
        }
        found
    }

    /// Remove a registered job from in-memory dispatch tables.
    pub async fn remove_job(&self, id: &str) -> bool {
        let removed = self.jobs.write().await.remove(id).is_some();
        if removed {
            self.last_dispatched.write().await.remove(id);
        }
        removed
    }

    /// Start the scheduler loop (runs indefinitely, should be spawned as a task).
    pub async fn run(self: Arc<Self>) {
        info!("scheduler started");
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        // Consume the first immediate tick so we do not attempt to catch up and
        // re-fire jobs that were scheduled before this process started.
        interval.tick().await;
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
                            // Find the most recent scheduled time that has already
                            // passed (within the last 61 seconds).  A 61-second
                            // look-back window accommodates the 60-second polling
                            // interval with a 1-second safety margin.
                            let window_start = now - chrono::Duration::seconds(61);
                            let most_recent_past = schedule
                                .after(&window_start)
                                .take_while(|t| *t <= now)
                                .last();

                            match most_recent_past {
                                None => false,
                                Some(scheduled_at) => {
                                    // Only fire if this job has not already been
                                    // dispatched at or after the scheduled time.
                                    let dispatched = self.last_dispatched.try_read();
                                    match dispatched {
                                        Ok(map) => map
                                            .get(id.as_str())
                                            .map(|last| *last < scheduled_at)
                                            .unwrap_or(true),
                                        Err(_) => {
                                            warn!(
                                                job_id = %id,
                                                "could not read last_dispatched map (lock contention); skipping job this tick"
                                            );
                                            false
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("invalid cron expression for job {}: {}", id, e);
                            false
                        }
                    };

                    if should_run {
                        // Record dispatch time before spawning so a concurrent tick
                        // that arrives before the job finishes does not fire again.
                        {
                            let mut map = self.last_dispatched.write().await;
                            map.insert(id.clone(), now);
                        }

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

                            if let Err(ref e) = result {
                                error!("job {} failed: {}", job_id, e);
                            }

                            let run_row = JobRunRow {
                                id: run_id,
                                job_id: job_id.clone(),
                                status,
                                output,
                                error,
                                started_at,
                                finished_at: Some(finished_at),
                            };

                            let repo = unly_db::repo::job::JobRepo::new(db.conn());
                            if let Err(e) = repo.insert_run(&run_row).await {
                                warn!("failed to persist job run: {}", e);
                            }
                            // Update last_run_at on the job row.
                            if let Err(e) = repo.update_last_run(&job_id, finished_at).await {
                                warn!("failed to update last_run_at for job {}: {}", job_id, e);
                            }
                        });
                    }
                }
            }
        }
    }
}
