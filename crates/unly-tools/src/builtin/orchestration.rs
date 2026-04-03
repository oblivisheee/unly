use async_trait::async_trait;
use chrono::Utc;
use cron::Schedule;
use once_cell::sync::Lazy;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Instant;
use unly_config::DbType;
use unly_core::ids::{AgentId, ChatId};
use unly_core::permissions::PermissionSet;
use unly_core::tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema};
use unly_core::Result;
use unly_db::Database;
use unly_scheduler::{JobDefinition, Scheduler};

type SubagentSpawner = Arc<
    dyn Fn(
            String,
            ChatId,
            Option<String>,
            Option<String>,
            PermissionSet,
            Option<AgentId>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send>,
        > + Send
        + Sync,
>;

type CronRunner = Arc<
    dyn Fn(
            String,
            ChatId,
            String,
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send>,
        > + Send
        + Sync,
>;

static SUBAGENT_EXECUTOR: Lazy<RwLock<Option<SubagentSpawner>>> = Lazy::new(|| RwLock::new(None));
static CRON_EXECUTOR: Lazy<RwLock<Option<CronRunner>>> = Lazy::new(|| RwLock::new(None));
static ACTIVE_SCHEDULER: Lazy<RwLock<Option<Arc<Scheduler>>>> = Lazy::new(|| RwLock::new(None));

pub fn register_subagent_executor(executor: SubagentSpawner) {
    if let Ok(mut guard) = SUBAGENT_EXECUTOR.write() {
        *guard = Some(executor);
    }
}

pub fn register_cron_executor(executor: CronRunner) {
    if let Ok(mut guard) = CRON_EXECUTOR.write() {
        *guard = Some(executor);
    }
}

pub fn set_active_scheduler(scheduler: Arc<Scheduler>) {
    if let Ok(mut guard) = ACTIVE_SCHEDULER.write() {
        *guard = Some(scheduler);
    }
}

pub struct SpawnSubagentTool;

#[async_trait]
impl Tool for SpawnSubagentTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "spawn_subagent".to_string(),
            description:
                "Create and start a subagent in background with full command/tool permissions."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type":"string","description":"Task for subagent"},
                    "provider": {"type":"string"},
                    "model": {"type":"string"}
                },
                "required": ["task"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let Some(task) = args.get("task").and_then(|v| v.as_str()).map(str::trim) else {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "missing task argument",
                start.elapsed().as_millis() as u64,
            ));
        };
        if task.is_empty() {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "task must not be empty",
                start.elapsed().as_millis() as u64,
            ));
        }
        let Some(chat_id) = ctx.chat_id else {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "spawn_subagent requires chat context",
                start.elapsed().as_millis() as u64,
            ));
        };
        let executor = SUBAGENT_EXECUTOR.read().ok().and_then(|g| g.clone());
        let Some(executor) = executor else {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "subagent executor not initialized",
                start.elapsed().as_millis() as u64,
            ));
        };
        let provider = args
            .get("provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let model = args
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        match executor(
            task.to_string(),
            chat_id,
            provider,
            model,
            PermissionSet::admin(),
            ctx.agent_id,
        )
        .await
        {
            Ok(id) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                json!({"status":"spawned","subagent_id":id,"task":task}).to_string(),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

#[derive(Clone)]
pub struct CronJobTool {
    db: Database,
    scheduler: Arc<Scheduler>,
}

impl CronJobTool {
    pub fn new(db: Database, scheduler: Arc<Scheduler>) -> Self {
        Self { db, scheduler }
    }
}

#[async_trait]
impl Tool for CronJobTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "cron_job".to_string(),
            description: "Manage scheduled cron jobs for background agent tasks (create/list/enable/disable/run_now/delete).".to_string(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "action":{"type":"string","enum":["create","list","enable","disable","run_now","delete"]},
                    "id":{"type":"string"},
                    "name":{"type":"string"},
                    "cron_expression":{"type":"string","description":"Cron schedule. Supports 5-field (min hour day month weekday) and 6-field (sec min hour day month weekday) formats."},
                    "task":{"type":"string"},
                    "notify_mode":{"type":"string","enum":["silent","message"],"default":"silent"}
                },
                "required":["action"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let Some(chat_id) = ctx.chat_id else {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "cron_job requires chat context",
                start.elapsed().as_millis() as u64,
            ));
        };
        match action {
            "create" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("job-{}", uuid::Uuid::new_v4()));
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Scheduled job")
                    .to_string();
                let cron_expression = match args.get("cron_expression").and_then(|v| v.as_str()) {
                    Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => {
                        return Ok(ToolResult::error(
                            ctx.tool_call_id.clone(),
                            "create requires cron_expression",
                            start.elapsed().as_millis() as u64,
                        ));
                    }
                };
                let cron_expression = normalize_cron_expression(&cron_expression);
                if let Err(e) = Schedule::from_str(&cron_expression) {
                    return Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        format!("invalid cron_expression '{}': {}", cron_expression, e),
                        start.elapsed().as_millis() as u64,
                    ));
                }
                let task = match args.get("task").and_then(|v| v.as_str()) {
                    Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                    _ => {
                        return Ok(ToolResult::error(
                            ctx.tool_call_id.clone(),
                            "create requires task",
                            start.elapsed().as_millis() as u64,
                        ));
                    }
                };
                let notify_mode = args
                    .get("notify_mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("silent")
                    .to_string();

                let payload = json!({
                    "task": task,
                    "chat_id": chat_id.to_string(),
                    "notify_mode": notify_mode,
                });
                let def = JobDefinition::cron(id.clone(), name.clone(), cron_expression, payload);
                let cron_exec = CRON_EXECUTOR.read().ok().and_then(|g| g.clone());
                let callback = Arc::new(move |payload: Value| {
                    let cron_exec = cron_exec.clone();
                    Box::pin(async move {
                        let Some(exec) = cron_exec else {
                            return Err("cron executor not initialized".to_string());
                        };
                        let task = payload
                            .get("task")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let chat_raw = payload
                            .get("chat_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        let chat_id = ChatId::from_str(chat_raw)
                            .map_err(|e| format!("invalid chat id: {}", e))?;
                        let notify_mode = payload
                            .get("notify_mode")
                            .and_then(|v| v.as_str())
                            .unwrap_or("silent")
                            .to_string();
                        exec(task, chat_id, notify_mode, "cron".to_string()).await
                    })
                        as std::pin::Pin<
                            Box<
                                dyn std::future::Future<
                                        Output = std::result::Result<String, String>,
                                    > + Send,
                            >,
                        >
                });
                self.scheduler.register(def, callback).await;
                Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    json!({"status":"created","id":id,"name":name}).to_string(),
                    start.elapsed().as_millis() as u64,
                ))
            }
            "list" => {
                let repo = unly_db::repo::job::JobRepo::new(self.db.conn());
                let rows = repo
                    .list_enabled()
                    .await
                    .map_err(|e| unly_core::Error::Database(e.to_string()))?;
                Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
                    start.elapsed().as_millis() as u64,
                ))
            }
            "enable" | "disable" | "delete" => {
                let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
                    return Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        format!("{} requires id", action),
                        start.elapsed().as_millis() as u64,
                    ));
                };
                let backend = match self.db.db_type() {
                    DbType::Postgres => DatabaseBackend::Postgres,
                    DbType::Sqlite => DatabaseBackend::Sqlite,
                };
                let sql = match action {
                    "enable" => format!(
                        "UPDATE jobs SET enabled=1, updated_at='{}' WHERE id='{}'",
                        Utc::now().to_rfc3339(),
                        escape_sql(id)
                    ),
                    "disable" => format!(
                        "UPDATE jobs SET enabled=0, updated_at='{}' WHERE id='{}'",
                        Utc::now().to_rfc3339(),
                        escape_sql(id)
                    ),
                    _ => format!("DELETE FROM jobs WHERE id='{}'", escape_sql(id)),
                };
                self.db
                    .conn()
                    .execute(Statement::from_string(backend, sql))
                    .await
                    .map_err(|e| unly_core::Error::Database(e.to_string()))?;
                if action == "enable" || action == "disable" {
                    let _ = self.scheduler.set_job_enabled(id, action == "enable").await;
                } else {
                    let _ = self.scheduler.remove_job(id).await;
                }
                Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    json!({"status":"ok","action":action,"id":id}).to_string(),
                    start.elapsed().as_millis() as u64,
                ))
            }
            "run_now" => {
                let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
                    return Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        "run_now requires id",
                        start.elapsed().as_millis() as u64,
                    ));
                };
                let backend = match self.db.db_type() {
                    DbType::Postgres => DatabaseBackend::Postgres,
                    DbType::Sqlite => DatabaseBackend::Sqlite,
                };
                let query = Statement::from_string(
                    backend,
                    format!(
                        "SELECT payload FROM jobs WHERE id='{}' LIMIT 1",
                        escape_sql(id)
                    ),
                );
                let row = self
                    .db
                    .conn()
                    .query_one(query)
                    .await
                    .map_err(|e| unly_core::Error::Database(e.to_string()))?;
                let Some(row) = row else {
                    return Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        format!("job '{}' not found", id),
                        start.elapsed().as_millis() as u64,
                    ));
                };
                let payload_raw: String = row
                    .try_get("", "payload")
                    .map_err(|e| unly_core::Error::Database(e.to_string()))?;
                let payload: Value = serde_json::from_str(&payload_raw).unwrap_or_default();
                let task = payload
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let chat_raw = payload
                    .get("chat_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let notify_mode = payload
                    .get("notify_mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("silent")
                    .to_string();
                let Some(exec) = CRON_EXECUTOR.read().ok().and_then(|g| g.clone()) else {
                    return Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        "cron executor not initialized",
                        start.elapsed().as_millis() as u64,
                    ));
                };
                let chat_id = match ChatId::from_str(chat_raw) {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(ToolResult::error(
                            ctx.tool_call_id.clone(),
                            format!("invalid stored chat_id: {}", e),
                            start.elapsed().as_millis() as u64,
                        ));
                    }
                };
                let out = exec(task, chat_id, notify_mode, "manual".to_string())
                    .await
                    .unwrap_or_else(|e| format!("job run failed: {}", e));
                Ok(ToolResult::success(
                    ctx.tool_call_id.clone(),
                    out,
                    start.elapsed().as_millis() as u64,
                ))
            }
            _ => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "invalid action. Use create|list|enable|disable|run_now|delete",
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

pub fn create_scheduler(db: Database, config: &unly_config::SchedulerConfig) -> Arc<Scheduler> {
    let scheduler = Arc::new(Scheduler::new(db, config.max_concurrent_jobs.max(1)));
    set_active_scheduler(scheduler.clone());
    scheduler
}

pub fn scheduler_ref() -> Option<Arc<Scheduler>> {
    ACTIVE_SCHEDULER.read().ok().and_then(|g| g.clone())
}

pub fn build_notify_message(task: &str, trigger: &str, result: &str) -> String {
    format!(
        "Cron job executed ({})\nTask: {}\nResult: {}",
        trigger, task, result
    )
}

pub fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}

fn normalize_cron_expression(raw: &str) -> String {
    let trimmed = raw.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    // Accept 5-field standard cron by prepending seconds=0 to match runtime parser.
    if parts.len() == 5 {
        format!("0 {}", trimmed)
    } else {
        trimmed.to_string()
    }
}
