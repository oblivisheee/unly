use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};
use tokio::sync::watch;
use tokio::sync::Semaphore;
use tokio::time::Duration;
use tracing::info;

use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseBackend, Set, Statement};
use unly_core::{ids::AgentId, permissions::PermissionSet, types::ExecutionStatus, Result};
use unly_db::entity::subagents;
use unly_db::Database;

use crate::context::AgentContext;
use crate::runtime::{AgentResponse, AgentRuntime};

static SUBAGENT_TASKS: LazyLock<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A request to spawn a subagent.
#[derive(Debug)]
pub struct SubagentRequest {
    pub goal: String,
    pub parent_agent_id: AgentId,
    pub depth: u32,
    pub permissions: PermissionSet,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub token_budget: u32,
}

/// Handle to a running subagent.
pub struct SubagentHandle {
    pub id: AgentId,
    pub status: ExecutionStatus,
    pub result: Option<AgentResponse>,
}

#[derive(Clone)]
pub struct SubagentSpawnConfig {
    pub max_depth: u32,
    pub max_concurrent: usize,
    pub max_children_per_parent: usize,
    pub token_budget: u32,
}

impl Default for SubagentSpawnConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_concurrent: 4,
            max_children_per_parent: 5,
            token_budget: 8192,
        }
    }
}

pub struct SubagentManager {
    semaphore: Arc<Semaphore>,
    config: SubagentSpawnConfig,
    db: Database,
}

impl SubagentManager {
    pub fn new(config: SubagentSpawnConfig, db: Database) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(config.max_concurrent.max(1))),
            config,
            db,
        }
    }

    pub async fn spawn_background(
        &self,
        request: SubagentRequest,
        runtime: Arc<AgentRuntime>,
        chat_id: unly_core::ids::ChatId,
    ) -> Result<SubagentHandle> {
        if request.depth >= self.config.max_depth {
            return Err(unly_core::Error::SubagentLimitExceeded);
        }
        let existing_children = self.count_children(request.parent_agent_id).await;
        if existing_children >= self.config.max_children_per_parent as u64 {
            return Err(unly_core::Error::Agent(format!(
                "child subagent limit reached for parent (max {})",
                self.config.max_children_per_parent
            )));
        }

        let agent_id = AgentId::new();
        let now = chrono::Utc::now().to_rfc3339();
        let row = subagents::ActiveModel {
            id: Set(agent_id.to_string()),
            parent_agent_id: Set(Some(request.parent_agent_id.to_string())),
            depth: Set((request.depth + 1) as i32),
            goal: Set(request.goal.clone()),
            status: Set("pending".to_string()),
            provider: Set(request.provider.clone()),
            model: Set(request.model.clone()),
            token_budget: Set(request.token_budget as i32),
            tokens_used: Set(0),
            result: Set(None),
            error: Set(None),
            chat_id: Set(Some(chat_id.to_string())),
            created_at: Set(now.clone()),
            updated_at: Set(now),
            finished_at: Set(None),
        };
        row.insert(self.db.conn())
            .await
            .map_err(|e| unly_core::Error::Database(e.to_string()))?;

        self.write_subagent_log(agent_id, "pending", &request.goal, None, None, None);

        let semaphore = self.semaphore.clone();
        let db = self.db.clone();
        let goal = request.goal.clone();
        let provider = request
            .provider
            .clone()
            .unwrap_or_else(|| "copilot".to_string());
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| "gpt-4o".to_string());
        let perms = request.permissions.clone();
        let depth = request.depth + 1;
        let token_budget = request.token_budget;
        let logs_goal = request.goal.clone();

        let task_handle = tokio::spawn(async move {
            let permit = match semaphore.acquire_owned().await {
                Ok(permit) => permit,
                Err(e) => {
                    let err = format!("failed to acquire subagent slot: {}", e);
                    let finished = chrono::Utc::now().to_rfc3339();
                    let _ = db
                        .conn()
                        .execute_unprepared(&format!(
                            "UPDATE subagents SET status='failed', error='{}', updated_at='{}', finished_at='{}' WHERE id='{}'",
                            escape_sql(&err),
                            finished,
                            finished,
                            agent_id
                        ))
                        .await;
                    append_subagent_log(agent_id, "failed", &logs_goal, None, Some(&err), None);
                    return;
                }
            };
            let _permit = permit;
            let started = chrono::Utc::now().to_rfc3339();
            let _ = db
                .conn()
                .execute_unprepared(&format!(
                    "UPDATE subagents SET status='running', updated_at='{}' WHERE id='{}'",
                    started, agent_id
                ))
                .await;

            append_subagent_log(agent_id, "running", &logs_goal, None, None, None);
            let (hb_tx, mut hb_rx) = watch::channel(false);
            let hb_db = db.clone();
            let hb_goal = logs_goal.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(15));
                let started_at = std::time::Instant::now();
                let mut tick: u64 = 0;
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            tick += 1;
                            let now = chrono::Utc::now().to_rfc3339();
                            let _ = hb_db.conn().execute_unprepared(&format!(
                                "UPDATE subagents SET updated_at='{}' WHERE id='{}' AND status='running'",
                                now,
                                agent_id
                            )).await;
                            let snapshot = read_subagent_runtime_snapshot(&agent_id.to_string());
                            let progress = if snapshot.is_empty() {
                                format!(
                                    "waiting for model/tool response (elapsed={}s, tick={})",
                                    started_at.elapsed().as_secs(),
                                    tick
                                )
                            } else {
                                format!(
                                    "{} (elapsed={}s, tick={})",
                                    snapshot,
                                    started_at.elapsed().as_secs(),
                                    tick
                                )
                            };
                            append_subagent_log(agent_id, "heartbeat", &hb_goal, Some(&progress), None, None);
                        }
                        changed = hb_rx.changed() => {
                            if changed.is_err() || *hb_rx.borrow() {
                                break;
                            }
                        }
                    }
                }
            });

            let mut ctx = AgentContext::new(
                chat_id,
                None,
                perms,
                provider,
                model,
                build_subagent_system_prompt(&goal, depth),
            );
            ctx.agent_id = agent_id;
            ctx.subagent_depth = depth;

            let run_result = tokio::time::timeout(
                Duration::from_secs(600),
                runtime.process(&mut ctx, goal.clone()),
            )
            .await;
            match run_result {
                Err(_) => {
                    let _ = hb_tx.send(true);
                    let finished = chrono::Utc::now().to_rfc3339();
                    let err = "subagent execution timed out after 600 seconds".to_string();
                    let _ = db
                        .conn()
                        .execute_unprepared(&format!(
                            "UPDATE subagents SET status='failed', error='{}', updated_at='{}', finished_at='{}' WHERE id='{}'",
                            escape_sql(&err),
                            finished,
                            finished,
                            agent_id
                        ))
                        .await;
                    append_subagent_log(
                        agent_id,
                        "failed",
                        &logs_goal,
                        None,
                        Some(&err),
                        Some(token_budget),
                    );
                }
                Ok(Ok(response)) => {
                    let _ = hb_tx.send(true);
                    let finished = chrono::Utc::now().to_rfc3339();
                    let result_text = match response {
                        AgentResponse::Text(t) => t,
                        AgentResponse::ApprovalRequired { pending } => {
                            // Subagents with admin permissions should have all tools
                            // pre-approved. If we still end up here (e.g. a Dangerous
                            // tool was called and the policy did not grant auto-approval),
                            // report it as a blocked result rather than silently failing.
                            let tool_names: Vec<&str> =
                                pending.iter().map(|p| p.tool_name.as_str()).collect();
                            let blocked_msg = format!(
                                "Subagent reached an approval gate for tools that could not be auto-approved at this depth. \
Blocked tool(s): {}. \
This subagent completed partial work up to this point; review the logs for progress made.",
                                tool_names.join(", ")
                            );
                            tracing::warn!(
                                subagent_id = %agent_id,
                                tools = ?tool_names,
                                "subagent reached unexpected approval gate; recording as partial result"
                            );
                            // Record as completed with partial result, not failed.
                            let _ = db
                                .conn()
                                .execute_unprepared(&format!(
                                    "UPDATE subagents SET status='completed', result='{}', updated_at='{}', finished_at='{}' WHERE id='{}'",
                                    escape_sql(&blocked_msg),
                                    finished,
                                    finished,
                                    agent_id
                                ))
                                .await;
                            append_subagent_log(
                                agent_id,
                                "completed_partial",
                                &logs_goal,
                                Some(&blocked_msg),
                                None,
                                Some(token_budget),
                            );
                            return;
                        }
                    };
                    let _ = db
                        .conn()
                        .execute_unprepared(&format!(
                            "UPDATE subagents SET status='completed', result='{}', updated_at='{}', finished_at='{}' WHERE id='{}'",
                            escape_sql(&result_text),
                            finished,
                            finished,
                            agent_id
                        ))
                        .await;
                    append_subagent_log(
                        agent_id,
                        "completed",
                        &logs_goal,
                        Some(&result_text),
                        None,
                        Some(token_budget),
                    );
                }
                Ok(Err(e)) => {
                    let _ = hb_tx.send(true);
                    let finished = chrono::Utc::now().to_rfc3339();
                    let err = e.to_string();
                    let _ = db
                        .conn()
                        .execute_unprepared(&format!(
                            "UPDATE subagents SET status='failed', error='{}', updated_at='{}', finished_at='{}' WHERE id='{}'",
                            escape_sql(&err),
                            finished,
                            finished,
                            agent_id
                        ))
                        .await;
                    append_subagent_log(
                        agent_id,
                        "failed",
                        &logs_goal,
                        None,
                        Some(&err),
                        Some(token_budget),
                    );
                }
            }
            if let Ok(mut map) = SUBAGENT_TASKS.lock() {
                map.remove(&agent_id.to_string());
            }
        });
        if let Ok(mut map) = SUBAGENT_TASKS.lock() {
            map.insert(agent_id.to_string(), task_handle);
        }

        Ok(SubagentHandle {
            id: agent_id,
            status: ExecutionStatus::Running,
            result: None,
        })
    }

    pub async fn subagent_status(&self, subagent_id: &str) -> Option<String> {
        let backend = match self.db.db_type() {
            unly_config::DbType::Postgres => DatabaseBackend::Postgres,
            unly_config::DbType::Sqlite => DatabaseBackend::Sqlite,
        };
        let stmt = Statement::from_string(
            backend,
            format!(
                "SELECT status FROM subagents WHERE id='{}' LIMIT 1",
                escape_sql(subagent_id)
            ),
        );
        let row = self.db.conn().query_one(stmt).await.ok().flatten()?;
        row.try_get("", "status").ok()
    }

    pub async fn stop_subagent(&self, subagent_id: &str) -> Result<()> {
        if let Ok(mut tasks) = SUBAGENT_TASKS.lock() {
            if let Some(handle) = tasks.remove(subagent_id) {
                handle.abort();
            }
        }
        let finished = chrono::Utc::now().to_rfc3339();
        self.db
            .conn()
            .execute_unprepared(&format!(
                "UPDATE subagents SET status='cancelled', error='cancelled by user', updated_at='{}', finished_at='{}' WHERE id='{}' AND (status='running' OR status='pending')",
                finished,
                finished,
                escape_sql(subagent_id)
            ))
            .await
            .map_err(|e| unly_core::Error::Database(e.to_string()))?;
        append_subagent_log_raw(
            subagent_id,
            "cancelled",
            "cancelled",
            None,
            Some("cancelled by user"),
            None,
        );
        Ok(())
    }

    async fn count_children(&self, parent_agent_id: AgentId) -> u64 {
        let backend = match self.db.db_type() {
            unly_config::DbType::Postgres => DatabaseBackend::Postgres,
            unly_config::DbType::Sqlite => DatabaseBackend::Sqlite,
        };
        let stmt = Statement::from_string(
            backend,
            format!(
                "SELECT COUNT(*) as cnt FROM subagents WHERE parent_agent_id='{}'",
                parent_agent_id
            ),
        );
        let row = self.db.conn().query_one(stmt).await.ok().flatten();
        if let Some(r) = row {
            if let Ok(cnt) = r.try_get::<i64>("", "cnt") {
                return cnt.max(0) as u64;
            }
        }
        0
    }

    fn write_subagent_log(
        &self,
        agent_id: AgentId,
        status: &str,
        goal: &str,
        result: Option<&str>,
        error: Option<&str>,
        token_budget: Option<u32>,
    ) {
        append_subagent_log(agent_id, status, goal, result, error, token_budget);
    }
}

/// Spawn a subagent with a given goal. The subagent runs the full agent loop
/// with a capped token budget and depth limit.
pub async fn spawn_subagent(
    request: SubagentRequest,
    runtime: Arc<AgentRuntime>,
    chat_id: unly_core::ids::ChatId,
) -> Result<SubagentHandle> {
    if request.depth >= 3 {
        return Err(unly_core::Error::SubagentLimitExceeded);
    }

    let agent_id = AgentId::new();
    info!(
        subagent_id = %agent_id,
        parent = %request.parent_agent_id,
        depth = request.depth,
        "spawning subagent"
    );

    let provider = request
        .provider
        .clone()
        .unwrap_or_else(|| "copilot".to_string());
    let model = request
        .model
        .clone()
        .unwrap_or_else(|| "gpt-4o".to_string());

    let mut ctx = AgentContext::new(
        chat_id,
        None,
        request.permissions,
        provider,
        model,
        build_subagent_system_prompt(&request.goal, request.depth + 1),
    );
    ctx.agent_id = agent_id;
    ctx.subagent_depth = request.depth + 1;

    let response = runtime.process(&mut ctx, request.goal.clone()).await?;

    Ok(SubagentHandle {
        id: agent_id,
        status: ExecutionStatus::Completed,
        result: Some(response),
    })
}

fn append_subagent_log(
    agent_id: AgentId,
    status: &str,
    goal: &str,
    result: Option<&str>,
    error: Option<&str>,
    token_budget: Option<u32>,
) {
    append_subagent_log_raw(
        &agent_id.to_string(),
        status,
        goal,
        result,
        error,
        token_budget,
    );
}

fn append_subagent_log_raw(
    agent_id: &str,
    status: &str,
    goal: &str,
    result: Option<&str>,
    error: Option<&str>,
    token_budget: Option<u32>,
) {
    let logs_dir = unly_config::workspace::subagent_logs_dir();
    let _ = std::fs::create_dir_all(&logs_dir);
    let path = logs_dir.join(format!("{}.log", agent_id));
    let mut entry = format!(
        "[{}] status={}\ngoal={}\n",
        chrono::Utc::now().to_rfc3339(),
        status,
        goal
    );
    if let Some(tb) = token_budget {
        entry.push_str(&format!("token_budget={}\n", tb));
    }
    if let Some(r) = result {
        entry.push_str(&format!("result={}\n", r));
    }
    if let Some(e) = error {
        entry.push_str(&format!("error={}\n", e));
    }
    entry.push('\n');
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, entry.as_bytes()));
}

fn read_subagent_runtime_snapshot(agent_id: &str) -> String {
    let path = unly_config::workspace::subagent_logs_dir().join(format!("{}.log", agent_id));
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let blocks: Vec<&str> = content.split("\n\n").collect();
    for block in blocks.into_iter().rev() {
        if block.contains("status=heartbeat") {
            continue;
        }
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("error=") {
                let v = v.trim();
                if !v.is_empty() {
                    return format!("last error: {}", v);
                }
            }
        }
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("result=") {
                let v = v.trim();
                if !v.is_empty() {
                    return format!("last result: {}", v);
                }
            }
        }
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("status=") {
                let v = v.trim();
                if !v.is_empty() {
                    return format!("phase: {}", v);
                }
            }
        }
    }
    String::new()
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}

fn build_subagent_system_prompt(goal: &str, depth: u32) -> String {
    if depth <= 1 {
        format!(
            "You are a level-1 coordinator subagent.\n\
Goal: {}\n\n\
Execution Framework:\n\
Stage 1 - PLAN:\n\
- Create a dependency-aware execution plan (ordered steps + parallel branches).\n\
- Mark which branches can run in parallel and where integration points exist.\n\
Stage 2 - ORCHESTRATE:\n\
- Execute directly where efficient.\n\
- Spawn deeper subagents only for independent or specialist branches.\n\
Stage 3 - INTEGRATE:\n\
- Merge child results, resolve inconsistencies, and validate deliverables.\n\
Stage 4 - FINALIZE:\n\
- Provide the real end result with concrete outputs/artifacts.\n\n\
Rules:\n\
- Do not stop at 'delegated' status messages.\n\
- Do not return only planning text.\n\
- Keep ownership of completion; child outputs are intermediate.\n\
- Return concise sections: PLAN, PARALLEL BRANCHES, WORK COMPLETED, FINAL RESULT, BLOCKERS (if any).",
            goal
        )
    } else {
        format!(
            "You are an execution subagent at depth {}.\n\
Goal: {}\n\n\
Execution Framework:\n\
1. Understand assigned scope and acceptance criteria.\n\
2. Execute scoped steps directly and efficiently.\n\
3. Validate your own output before reporting.\n\
4. Return structured, merge-ready output.\n\n\
Rules:\n\
- Do not re-delegate unless absolutely necessary.\n\
- Return sections: INPUT SCOPE, ACTIONS PERFORMED, OUTPUT, RISKS/BLOCKERS.",
            depth, goal
        )
    }
}
