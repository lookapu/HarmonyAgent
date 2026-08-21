//! SQLite 持久化任务调度器：进程内 Future 负责执行，数据库队列负责所有权、优先级、
//! 租约、检查点与重启后的安全接管。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub task_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub goal: String,
    pub state: String,
    pub priority: i64,
    pub worker_id: Option<String>,
    pub attempt: i64,
    pub max_attempts: i64,
    pub lease_expires_at: Option<i64>,
    pub budget_json: String,
    pub checkpoint_json: String,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    pub payload_json: String,
    pub resume_token: Option<String>,
    pub claimed_at: Option<i64>,
    pub last_checkpoint_at: Option<i64>,
    pub next_attempt_at: Option<i64>,
    pub concurrency_key: Option<String>,
    pub tenant_id: String,
    pub lease_token: Option<String>,
    pub claim_epoch: i64,
    pub last_worker_id: Option<String>,
    pub recovery_count: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub worker_id: String,
    pub worker_kind: String,
    pub pid: i64,
    pub hostname: String,
    pub version: String,
    pub state: String,
    pub capacity: i64,
    pub active_tasks: i64,
    pub started_at: i64,
    pub last_heartbeat_at: i64,
    pub draining_at: Option<i64>,
    pub stopped_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct WorkerRuntimeStats {
    pub active_workers: i64,
    pub lost_workers: i64,
    pub running_tasks: i64,
    pub recovered_tasks: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnqueueSpec {
    pub run_id: String,
    pub conversation_id: String,
    pub goal: String,
    pub priority: i64,
    pub max_attempts: i64,
    pub concurrency_key: Option<String>,
    pub payload: serde_json::Value,
    pub budget: serde_json::Value,
}

static WORKER_ID: OnceLock<String> = OnceLock::new();

pub fn current_worker_id() -> &'static str {
    WORKER_ID
        .get_or_init(|| format!("desktop:{}:{}", std::process::id(), uuid::Uuid::new_v4()))
        .as_str()
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "localhost".into())
}

pub fn register_current_worker(conn: &Connection) -> Result<String, String> {
    register_worker(conn, current_worker_id(), "desktop", 8)?;
    Ok(current_worker_id().to_string())
}

pub fn register_worker(
    conn: &Connection,
    worker_id: &str,
    worker_kind: &str,
    capacity: i64,
) -> Result<(), String> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO agent_workers
         (worker_id,worker_kind,pid,hostname,version,state,capacity,active_tasks,started_at,last_heartbeat_at,metadata_json)
         VALUES (?1,?2,?3,?4,?5,'active',?6,0,?7,?7,'{}')
         ON CONFLICT(worker_id) DO UPDATE SET state='active',capacity=excluded.capacity,
         last_heartbeat_at=excluded.last_heartbeat_at,stopped_at=NULL",
        params![worker_id,worker_kind,std::process::id() as i64,hostname(),env!("CARGO_PKG_VERSION"),
            capacity.clamp(1,64),now],
    ).map_err(|e|e.to_string())?;
    Ok(())
}

pub fn heartbeat_worker(conn: &Connection, worker_id: &str) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE agent_workers SET last_heartbeat_at=?1,
         active_tasks=(SELECT COUNT(*) FROM agent_task_queue WHERE worker_id=?2 AND state='running')
         WHERE worker_id=?2 AND state IN ('active','draining')",
            params![now_ms(), worker_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

pub fn stop_current_worker(conn: &Connection) -> Result<(), String> {
    let now = now_ms();
    conn.execute(
        "UPDATE agent_workers SET state='stopped',active_tasks=0,stopped_at=?1,last_heartbeat_at=?1
         WHERE worker_id=?2",
        params![now, current_worker_id()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_workers(conn: &Connection, limit: usize) -> Result<Vec<WorkerInfo>, String> {
    let mut stmt = conn.prepare(
        "SELECT worker_id,worker_kind,pid,hostname,version,state,capacity,active_tasks,
         started_at,last_heartbeat_at,draining_at,stopped_at FROM agent_workers
         ORDER BY CASE state WHEN 'active' THEN 0 WHEN 'draining' THEN 1 ELSE 2 END,last_heartbeat_at DESC LIMIT ?1",
    ).map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map([limit.clamp(1, 500) as i64], |row| {
            Ok(WorkerInfo {
                worker_id: row.get(0)?,
                worker_kind: row.get(1)?,
                pid: row.get(2)?,
                hostname: row.get(3)?,
                version: row.get(4)?,
                state: row.get(5)?,
                capacity: row.get(6)?,
                active_tasks: row.get(7)?,
                started_at: row.get(8)?,
                last_heartbeat_at: row.get(9)?,
                draining_at: row.get(10)?,
                stopped_at: row.get(11)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 将已创建的 durable run 放入可认领队列。敏感凭据不得进入 payload；恢复时重新从
/// Provider 配置读取。concurrency_key 保证同一工程/会话不会被多个 worker 并发修改。
pub fn enqueue(conn: &Connection, spec: &EnqueueSpec) -> Result<String, String> {
    let task_id = format!("task:{}", spec.run_id);
    let now = now_ms();
    let resume_token = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO agent_task_queue
         (task_id,run_id,conversation_id,goal,state,priority,attempt,max_attempts,budget_json,
          checkpoint_json,payload_json,resume_token,concurrency_key,tenant_id,created_at,updated_at)
         VALUES (?1,?2,?3,?4,'queued',?5,0,?6,?7,'{}',?8,?9,?10,'local',?11,?11)
         ON CONFLICT(run_id) DO UPDATE SET state=CASE WHEN agent_task_queue.state IN
          ('completed','failed','cancelled') THEN agent_task_queue.state ELSE 'queued' END,
          priority=excluded.priority,max_attempts=excluded.max_attempts,budget_json=excluded.budget_json,
          payload_json=excluded.payload_json,resume_token=excluded.resume_token,
          concurrency_key=excluded.concurrency_key,updated_at=excluded.updated_at",
        params![task_id,spec.run_id,spec.conversation_id,spec.goal,spec.priority.clamp(0,100),
            spec.max_attempts.clamp(1,20),spec.budget.to_string(),spec.payload.to_string(),resume_token,
            spec.concurrency_key,now],
    ).map_err(|e| e.to_string())?;
    Ok(task_id)
}

/// 原子认领一个就绪任务。条件更新保证竞争者至多一个成功；concurrency_key 已有
/// 活跃所有者时跳过，避免同一工作区发生并发副作用。
pub fn claim_next(conn: &Connection, lease_ms: i64) -> Result<Option<ScheduledTask>, String> {
    register_current_worker(conn)?;
    claim_next_for_worker(conn, current_worker_id(), lease_ms)
}

pub fn claim_next_for_worker(
    conn: &Connection,
    worker_id: &str,
    lease_ms: i64,
) -> Result<Option<ScheduledTask>, String> {
    let now = now_ms();
    let available = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_workers WHERE worker_id=?1 AND state='active' AND active_tasks<capacity)",
        [worker_id], |row| row.get::<_,bool>(0),
    ).map_err(|e|e.to_string())?;
    if !available {
        return Ok(None);
    }
    let lease_token = uuid::Uuid::new_v4().to_string();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let task_id = tx
        .query_row(
            "UPDATE agent_task_queue SET state='running',worker_id=?1,last_worker_id=?1,
             attempt=attempt+1,claim_epoch=claim_epoch+1,lease_token=?2,claimed_at=?3,
             lease_expires_at=?4,next_attempt_at=NULL,error=NULL,updated_at=?3
             WHERE task_id=(SELECT q.task_id FROM agent_task_queue q
               WHERE q.state IN ('queued','recovery_required')
                 AND (q.next_attempt_at IS NULL OR q.next_attempt_at<=?3)
                 AND q.attempt<q.max_attempts
                 AND (q.concurrency_key IS NULL OR NOT EXISTS(
                   SELECT 1 FROM agent_task_queue active WHERE active.concurrency_key=q.concurrency_key
                   AND active.state='running' AND active.task_id!=q.task_id))
               ORDER BY q.priority DESC,q.created_at ASC LIMIT 1)
             AND state IN ('queued','recovery_required')
             AND EXISTS(SELECT 1 FROM agent_workers WHERE worker_id=?1 AND state='active' AND active_tasks<capacity)
             RETURNING task_id",
            params![worker_id,lease_token,now,now.saturating_add(lease_ms.max(10_000))],
            |row| row.get::<_, String>(0),
        )
        .optional().map_err(|e|e.to_string())?;
    let Some(task_id) = task_id else {
        tx.commit().map_err(|e| e.to_string())?;
        return Ok(None);
    };
    tx.execute(
        "INSERT INTO agent_task_attempts
         (task_id,attempt,worker_id,lease_token,state,checkpoint_json,started_at,last_heartbeat_at)
         SELECT task_id,attempt,worker_id,lease_token,'running',checkpoint_json,?1,?1
         FROM agent_task_queue WHERE task_id=?2",
        params![now, task_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE agent_workers SET active_tasks=active_tasks+1,last_heartbeat_at=?1
         WHERE worker_id=?2 AND state='active'",
        params![now, worker_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    get_by_task_id(conn, &task_id)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn register_active(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    goal: &str,
    priority: i64,
    budget: &serde_json::Value,
    lease_ms: i64,
) -> Result<String, String> {
    let task_id = enqueue(
        conn,
        &EnqueueSpec {
            run_id: run_id.into(),
            conversation_id: conversation_id.into(),
            goal: goal.into(),
            priority,
            max_attempts: 3,
            concurrency_key: Some(format!("conversation:{conversation_id}")),
            payload: serde_json::json!({"resume_run_id":serde_json::Value::Null}),
            budget: budget.clone(),
        },
    )?;
    let now = now_ms();
    register_current_worker(conn)?;
    let lease_token = uuid::Uuid::new_v4().to_string();
    let changed = conn
        .execute(
            "UPDATE agent_task_queue SET state='running',worker_id=?1,last_worker_id=?1,
         attempt=CASE WHEN attempt=0 THEN 1 ELSE attempt END,claim_epoch=claim_epoch+1,
         lease_token=?2,claimed_at=COALESCE(claimed_at,?3),lease_expires_at=?4,updated_at=?3
         WHERE task_id=?5 AND state IN ('queued','recovery_required')
         AND EXISTS(SELECT 1 FROM agent_workers WHERE worker_id=?1 AND state='active' AND active_tasks<capacity)
         AND (concurrency_key IS NULL OR NOT EXISTS(SELECT 1 FROM agent_task_queue active
           WHERE active.concurrency_key=agent_task_queue.concurrency_key AND active.state='running'
           AND active.task_id!=agent_task_queue.task_id))",
            params![
                current_worker_id(),
                lease_token,
                now,
                now.saturating_add(lease_ms.max(10_000)),
                task_id
            ],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("任务已被其他 Worker 认领".into());
    }
    conn.execute(
        "INSERT OR REPLACE INTO agent_task_attempts
         (task_id,attempt,worker_id,lease_token,state,checkpoint_json,started_at,last_heartbeat_at)
         SELECT task_id,attempt,worker_id,lease_token,'running',checkpoint_json,?1,?1
         FROM agent_task_queue WHERE task_id=?2",
        params![now, task_id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE agent_workers SET active_tasks=active_tasks+1,last_heartbeat_at=?1 WHERE worker_id=?2",
        params![now,current_worker_id()],
    ).map_err(|e|e.to_string())?;
    conn.execute(
        "UPDATE agent_runs SET scheduler_task_id=?1,budget_json=?2 WHERE run_id=?3",
        params![task_id, budget.to_string(), run_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(task_id)
}

pub fn checkpoint(
    conn: &Connection,
    run_id: &str,
    checkpoint: &serde_json::Value,
    lease_ms: i64,
) -> Result<(), String> {
    let task = get(conn, run_id)?.ok_or_else(|| "调度任务不存在".to_string())?;
    let token = task
        .lease_token
        .ok_or_else(|| "任务没有有效租约".to_string())?;
    checkpoint_owned(
        conn,
        run_id,
        current_worker_id(),
        &token,
        checkpoint,
        lease_ms,
    )
}

pub fn checkpoint_owned(
    conn: &Connection,
    run_id: &str,
    worker_id: &str,
    lease_token: &str,
    checkpoint: &serde_json::Value,
    lease_ms: i64,
) -> Result<(), String> {
    let now = now_ms();
    let expires = now.saturating_add(lease_ms.max(10_000));
    let changed = conn.execute(
        "UPDATE agent_task_queue SET checkpoint_json=?1,lease_expires_at=?2,last_checkpoint_at=?3,updated_at=?3
         WHERE run_id=?4 AND state='running' AND worker_id=?5 AND lease_token=?6",
        params![
            checkpoint.to_string(),
            expires,
            now,
            run_id,
            worker_id,
            lease_token,
        ],
    )
    .map_err(|e| e.to_string())?;
    if changed == 0 {
        let _ = crate::agent::enterprise::audit(
            conn, Some(run_id), None, worker_id, "lease.fence_rejected",
            "agent_task_queue", "blocked",
            &serde_json::json!({"operation":"checkpoint"}),
        );
        return Err("STALE_LEASE: Worker 已失去任务所有权，拒绝写入检查点".into());
    }
    conn.execute(
        "UPDATE agent_task_attempts SET checkpoint_json=?1,last_heartbeat_at=?2
         WHERE task_id=(SELECT task_id FROM agent_task_queue WHERE run_id=?3)
         AND worker_id=?4 AND lease_token=?5 AND state='running'",
        params![checkpoint.to_string(), now, run_id, worker_id, lease_token],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn renew_owned(
    conn: &Connection,
    run_id: &str,
    worker_id: &str,
    lease_ms: i64,
) -> Result<bool, String> {
    let now = now_ms();
    let expires = now.saturating_add(lease_ms.max(10_000));
    let changed = conn
        .execute(
            "UPDATE agent_task_queue SET lease_expires_at=?1,updated_at=?2
         WHERE run_id=?3 AND state='running' AND worker_id=?4",
            params![expires, now, run_id, worker_id],
        )
        .map_err(|e| e.to_string())?;
    if changed > 0 {
        conn.execute(
            "UPDATE agent_task_attempts SET last_heartbeat_at=?1 WHERE task_id=(SELECT task_id FROM agent_task_queue WHERE run_id=?2)
             AND worker_id=?3 AND state='running'",
            params![now,run_id,worker_id],
        ).map_err(|e|e.to_string())?;
    }
    Ok(changed > 0)
}

pub fn release_for_retry(
    conn: &Connection,
    run_id: &str,
    error: &str,
    delay_ms: i64,
) -> Result<bool, String> {
    let now = now_ms();
    let changed = conn.execute(
        "UPDATE agent_task_queue SET state=CASE WHEN attempt>=max_attempts THEN 'failed' ELSE 'queued' END,
         lease_expires_at=NULL,next_attempt_at=CASE WHEN attempt>=max_attempts THEN NULL ELSE ?1 END,
         error=?2,updated_at=?3,finished_at=CASE WHEN attempt>=max_attempts THEN ?3 ELSE NULL END,
         lease_token=NULL,last_worker_id=worker_id,worker_id=NULL
         WHERE run_id=?4 AND state='running' AND worker_id=?5",
        params![now.saturating_add(delay_ms.max(0)),error,now,run_id,current_worker_id()],
    ).map_err(|e| e.to_string())?;
    if changed > 0 {
        conn.execute(
            "UPDATE agent_task_attempts SET state='retry_released',error=?1,finished_at=?2,last_heartbeat_at=?2
             WHERE task_id=(SELECT task_id FROM agent_task_queue WHERE run_id=?3) AND state='running'",
            params![error,now,run_id],
        ).map_err(|e|e.to_string())?;
        conn.execute(
            "UPDATE agent_workers SET active_tasks=MAX(active_tasks-1,0) WHERE worker_id=?1",
            [current_worker_id()],
        ).map_err(|e|e.to_string())?;
    }
    Ok(changed > 0)
}

pub fn request_resume(conn: &Connection, run_id: &str, resume_token: &str) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE agent_task_queue SET state='queued',worker_id=NULL,lease_token=NULL,lease_expires_at=NULL,
         next_attempt_at=?1,error=NULL,updated_at=?1 WHERE run_id=?2 AND resume_token=?3
         AND state='recovery_required' AND attempt<max_attempts",
            params![now_ms(), run_id, resume_token],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

pub fn mark_recovery_required(conn: &Connection, run_id: &str, error: &str) -> Result<(), String> {
    let now = now_ms();
    let changed = conn.execute(
        "UPDATE agent_task_queue SET state='recovery_required',worker_id=NULL,lease_expires_at=NULL,
         next_attempt_at=NULL,error=?1,updated_at=?2,lease_token=NULL,last_worker_id=worker_id
         WHERE run_id=?3
         AND state NOT IN ('completed','cancelled') AND (worker_id=?4 OR worker_id IS NULL)",
        params![error,now,run_id,current_worker_id()],
    ).map_err(|e| e.to_string())?;
    if changed > 0 {
        conn.execute(
            "UPDATE agent_task_attempts SET state='recovery_required',error=?1,finished_at=?2,last_heartbeat_at=?2
             WHERE task_id=(SELECT task_id FROM agent_task_queue WHERE run_id=?3) AND state='running'",
            params![error,now,run_id],
        ).map_err(|e|e.to_string())?;
        conn.execute(
            "UPDATE agent_workers SET active_tasks=MAX(active_tasks-1,0) WHERE worker_id=?1",
            [current_worker_id()],
        ).map_err(|e|e.to_string())?;
    }
    Ok(())
}

pub fn supersede_recovery(
    conn: &Connection,
    parent_run_id: &str,
    child_run_id: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_task_queue SET state='superseded',finished_at=?1,updated_at=?1,
         error='已由恢复分支接管：'||?2 WHERE run_id=?3 AND state='recovery_required'",
        params![now_ms(), child_run_id, parent_run_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn update_budget(
    conn: &Connection,
    run_id: &str,
    budget: &serde_json::Value,
) -> Result<(), String> {
    let encoded = budget.to_string();
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE agent_task_queue SET budget_json=?1,updated_at=?2 WHERE run_id=?3 AND state='running'",
        params![encoded, now, run_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE agent_runs SET budget_json=?1 WHERE run_id=?2",
        params![encoded, run_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub fn update_payload(
    conn: &Connection,
    run_id: &str,
    payload: &serde_json::Value,
) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_task_queue SET payload_json=?1,updated_at=?2 WHERE run_id=?3 AND state='running'",
        params![payload.to_string(),now_ms(),run_id],
    ).map_err(|e|e.to_string())?;
    Ok(())
}

pub fn finish(
    conn: &Connection,
    run_id: &str,
    state: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let now = now_ms();
    let changed = conn
        .execute(
            "UPDATE agent_task_queue SET state=?1,error=?2,lease_expires_at=NULL,lease_token=NULL,
         last_worker_id=worker_id,worker_id=NULL,updated_at=?3,finished_at=?3
         WHERE run_id=?4 AND state NOT IN ('completed','failed','cancelled') AND worker_id=?5",
            params![state, error, now, run_id, current_worker_id()],
        )
        .map_err(|e| e.to_string())?;
    if changed > 0 {
        conn.execute(
            "UPDATE agent_task_attempts SET state=?1,error=?2,finished_at=?3,last_heartbeat_at=?3
             WHERE task_id=(SELECT task_id FROM agent_task_queue WHERE run_id=?4) AND state='running'",
            params![state,error,now,run_id],
        ).map_err(|e|e.to_string())?;
        conn.execute(
            "UPDATE agent_workers SET active_tasks=MAX(active_tasks-1,0) WHERE worker_id=?1",
            [current_worker_id()],
        ).map_err(|e|e.to_string())?;
    }
    Ok(())
}

/// 仅回收真正过期的执行所有权；任务是否重放由 Recovery Orchestrator 决定。
pub fn recover_expired(conn: &Connection) -> Result<usize, String> {
    recover_stale_owners(conn, 60_000)
}

pub fn recover_orphaned_on_startup(conn: &Connection) -> Result<usize, String> {
    recover_stale_owners(conn, 60_000)
}

/// 回收租约过期、Owner 未注册或 Worker 心跳失联的任务。存活 Worker 的任务不会因另一
/// 个桌面实例启动而被误伤；旧 attempt 明确记为 lost，后续认领获得新的 fencing token。
pub fn recover_stale_owners(conn: &Connection, stale_after_ms: i64) -> Result<usize, String> {
    let now = now_ms();
    let cutoff = now.saturating_sub(stale_after_ms.max(0));
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE agent_workers SET state='lost',active_tasks=0 WHERE state IN ('active','draining')
         AND last_heartbeat_at<?1",
        [cutoff],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE agent_task_attempts SET state='lost',error=COALESCE(error,'Worker 心跳失联'),
         finished_at=?1,last_heartbeat_at=?1 WHERE state='running' AND task_id IN (
           SELECT q.task_id FROM agent_task_queue q LEFT JOIN agent_workers w ON w.worker_id=q.worker_id
           WHERE q.state='running' AND (q.lease_expires_at<?1 OR w.worker_id IS NULL
             OR w.state IN ('lost','stopped') OR w.last_heartbeat_at<?2))",
        params![now,cutoff],
    ).map_err(|e|e.to_string())?;
    let changed = tx.execute(
        "UPDATE agent_task_queue SET state='recovery_required',last_worker_id=worker_id,
         worker_id=NULL,lease_token=NULL,lease_expires_at=NULL,recovery_count=recovery_count+1,
         error=COALESCE(error,'Worker 失联或租约过期，等待安全恢复'),updated_at=?1
         WHERE state='running' AND (lease_expires_at<?1 OR worker_id NOT IN (
           SELECT worker_id FROM agent_workers WHERE state IN ('active','draining') AND last_heartbeat_at>=?2))",
        params![now,cutoff],
    ).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    if changed > 0 {
        let _ = crate::agent::enterprise::audit(
            conn, None, None, "kernel", "worker.recover", "agent_task_queue", "success",
            &serde_json::json!({"recovered_tasks":changed,"stale_after_ms":stale_after_ms}),
        );
    }
    Ok(changed)
}

pub fn runtime_stats(conn: &Connection) -> Result<WorkerRuntimeStats, String> {
    let (active_workers, lost_workers) = conn
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN state IN ('active','draining') THEN 1 ELSE 0 END),0),
         COALESCE(SUM(CASE WHEN state='lost' THEN 1 ELSE 0 END),0) FROM agent_workers",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let (running_tasks, recovered_tasks) = conn
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN state='running' THEN 1 ELSE 0 END),0),
         COALESCE(SUM(recovery_count),0) FROM agent_task_queue",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    Ok(WorkerRuntimeStats {
        active_workers,
        lost_workers,
        running_tasks,
        recovered_tasks,
    })
}

pub fn list(conn: &Connection, limit: usize) -> Result<Vec<ScheduledTask>, String> {
    let mut stmt = conn.prepare(
        "SELECT task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
                lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at,
                payload_json,resume_token,claimed_at,last_checkpoint_at,next_attempt_at,concurrency_key,tenant_id,
                lease_token,claim_epoch,last_worker_id,recovery_count
         FROM agent_task_queue ORDER BY
           CASE state WHEN 'running' THEN 0 WHEN 'queued' THEN 1 WHEN 'recovery_required' THEN 2 ELSE 3 END,
           priority DESC,created_at DESC LIMIT ?1",
    ).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit.clamp(1, 500) as i64], |row| {
            Ok(ScheduledTask {
                task_id: row.get(0)?,
                run_id: row.get(1)?,
                conversation_id: row.get(2)?,
                goal: row.get(3)?,
                state: row.get(4)?,
                priority: row.get(5)?,
                worker_id: row.get(6)?,
                attempt: row.get(7)?,
                max_attempts: row.get(8)?,
                lease_expires_at: row.get(9)?,
                budget_json: row.get(10)?,
                checkpoint_json: row.get(11)?,
                error: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
                finished_at: row.get(15)?,
                payload_json: row.get(16)?,
                resume_token: row.get(17)?,
                claimed_at: row.get(18)?,
                last_checkpoint_at: row.get(19)?,
                next_attempt_at: row.get(20)?,
                concurrency_key: row.get(21)?,
                tenant_id: row.get(22)?,
                lease_token: row.get(23)?,
                claim_epoch: row.get(24)?,
                last_worker_id: row.get(25)?,
                recovery_count: row.get(26)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn get(conn: &Connection, run_id: &str) -> Result<Option<ScheduledTask>, String> {
    conn.query_row(
        "SELECT task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
                lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at,
                payload_json,resume_token,claimed_at,last_checkpoint_at,next_attempt_at,concurrency_key,tenant_id,
                lease_token,claim_epoch,last_worker_id,recovery_count
         FROM agent_task_queue WHERE run_id=?1",
        [run_id],
        |row| {
            Ok(ScheduledTask {
                task_id: row.get(0)?,
                run_id: row.get(1)?,
                conversation_id: row.get(2)?,
                goal: row.get(3)?,
                state: row.get(4)?,
                priority: row.get(5)?,
                worker_id: row.get(6)?,
                attempt: row.get(7)?,
                max_attempts: row.get(8)?,
                lease_expires_at: row.get(9)?,
                budget_json: row.get(10)?,
                checkpoint_json: row.get(11)?,
                error: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
                finished_at: row.get(15)?,
                payload_json: row.get(16)?, resume_token: row.get(17)?, claimed_at: row.get(18)?,
                last_checkpoint_at: row.get(19)?, next_attempt_at: row.get(20)?,
                concurrency_key: row.get(21)?, tenant_id: row.get(22)?,
                lease_token: row.get(23)?, claim_epoch: row.get(24)?,
                last_worker_id: row.get(25)?, recovery_count: row.get(26)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn get_by_task_id(conn: &Connection, task_id: &str) -> Result<Option<ScheduledTask>, String> {
    conn.query_row(
        "SELECT task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
         lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at,
         payload_json,resume_token,claimed_at,last_checkpoint_at,next_attempt_at,concurrency_key,tenant_id,
         lease_token,claim_epoch,last_worker_id,recovery_count
         FROM agent_task_queue WHERE task_id=?1", [task_id], row_to_task,
    ).optional().map_err(|e| e.to_string())
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTask> {
    Ok(ScheduledTask {
        task_id: row.get(0)?,
        run_id: row.get(1)?,
        conversation_id: row.get(2)?,
        goal: row.get(3)?,
        state: row.get(4)?,
        priority: row.get(5)?,
        worker_id: row.get(6)?,
        attempt: row.get(7)?,
        max_attempts: row.get(8)?,
        lease_expires_at: row.get(9)?,
        budget_json: row.get(10)?,
        checkpoint_json: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        finished_at: row.get(15)?,
        payload_json: row.get(16)?,
        resume_token: row.get(17)?,
        claimed_at: row.get(18)?,
        last_checkpoint_at: row.get(19)?,
        next_attempt_at: row.get(20)?,
        concurrency_key: row.get(21)?,
        tenant_id: row.get(22)?,
        lease_token: row.get(23)?,
        claim_epoch: row.get(24)?,
        last_worker_id: row.get(25)?,
        recovery_count: row.get(26)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY,scheduler_task_id TEXT,budget_json TEXT); CREATE TABLE conversations(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c'); CREATE TABLE agent_task_queue(task_id TEXT PRIMARY KEY,run_id TEXT UNIQUE,conversation_id TEXT,goal TEXT,state TEXT,priority INTEGER,worker_id TEXT,attempt INTEGER,max_attempts INTEGER,lease_expires_at INTEGER,budget_json TEXT,checkpoint_json TEXT,error TEXT,created_at INTEGER,updated_at INTEGER,finished_at INTEGER,payload_json TEXT NOT NULL DEFAULT '{}',resume_token TEXT,claimed_at INTEGER,last_checkpoint_at INTEGER,next_attempt_at INTEGER,concurrency_key TEXT,tenant_id TEXT NOT NULL DEFAULT 'local',lease_token TEXT,claim_epoch INTEGER NOT NULL DEFAULT 0,last_worker_id TEXT,recovery_count INTEGER NOT NULL DEFAULT 0); CREATE TABLE agent_workers(worker_id TEXT PRIMARY KEY,worker_kind TEXT,pid INTEGER,hostname TEXT,version TEXT,state TEXT,capacity INTEGER,active_tasks INTEGER,started_at INTEGER,last_heartbeat_at INTEGER,draining_at INTEGER,stopped_at INTEGER,metadata_json TEXT); CREATE TABLE agent_task_attempts(task_id TEXT,attempt INTEGER,worker_id TEXT,lease_token TEXT,state TEXT,checkpoint_json TEXT,error TEXT,started_at INTEGER,last_heartbeat_at INTEGER,finished_at INTEGER,PRIMARY KEY(task_id,attempt));").unwrap();
        conn
    }
    #[test]
    fn task_lifecycle_is_durable() {
        let conn = conn();
        conn.execute("INSERT INTO agent_runs(run_id) VALUES('r')", [])
            .unwrap();
        register_active(
            &conn,
            "r",
            "c",
            "goal",
            80,
            &serde_json::json!({"rounds":40}),
            60_000,
        )
        .unwrap();
        checkpoint(&conn, "r", &serde_json::json!({"step":3}), 60_000).unwrap();
        assert!(get(&conn, "r")
            .unwrap()
            .unwrap()
            .checkpoint_json
            .contains("step"));
        update_budget(
            &conn,
            "r",
            &serde_json::json!({"effective_tool_rounds": 80}),
        )
        .unwrap();
        assert!(get(&conn, "r").unwrap().unwrap().budget_json.contains("80"));
        finish(&conn, "r", "completed", None).unwrap();
        assert_eq!(get(&conn, "r").unwrap().unwrap().state, "completed");
    }

    #[test]
    fn queue_claim_retry_and_resume_are_atomic() {
        let conn = conn();
        conn.execute("INSERT INTO agent_runs(run_id) VALUES('q')", [])
            .unwrap();
        enqueue(
            &conn,
            &EnqueueSpec {
                run_id: "q".into(),
                conversation_id: "c".into(),
                goal: "g".into(),
                priority: 90,
                max_attempts: 3,
                concurrency_key: Some("workspace:a".into()),
                payload: serde_json::json!({"safe":true}),
                budget: serde_json::json!({}),
            },
        )
        .unwrap();
        let claimed = claim_next(&conn, 60_000).unwrap().unwrap();
        assert_eq!(claimed.run_id, "q");
        assert_eq!(claimed.attempt, 1);
        assert!(claim_next(&conn, 60_000).unwrap().is_none());
        assert!(release_for_retry(&conn, "q", "transient", 0).unwrap());
        assert_eq!(claim_next(&conn, 60_000).unwrap().unwrap().attempt, 2);
        mark_recovery_required(&conn, "q", "restart").unwrap();
        let task = get(&conn, "q").unwrap().unwrap();
        assert!(request_resume(&conn, "q", task.resume_token.as_deref().unwrap()).unwrap());
        assert_eq!(get(&conn, "q").unwrap().unwrap().state, "queued");
    }

    #[test]
    fn stale_worker_is_fenced_after_takeover() {
        let conn = conn();
        conn.execute("INSERT INTO agent_runs(run_id) VALUES('f')", [])
            .unwrap();
        enqueue(
            &conn,
            &EnqueueSpec {
                run_id: "f".into(),
                conversation_id: "c".into(),
                goal: "g".into(),
                priority: 50,
                max_attempts: 3,
                concurrency_key: None,
                payload: serde_json::json!({}),
                budget: serde_json::json!({}),
            },
        )
        .unwrap();
        register_worker(&conn, "worker-old", "test", 1).unwrap();
        let old = claim_next_for_worker(&conn, "worker-old", 10_000)
            .unwrap()
            .unwrap();
        conn.execute(
            "UPDATE agent_workers SET last_heartbeat_at=0 WHERE worker_id='worker-old'",
            [],
        )
        .unwrap();
        recover_stale_owners(&conn, 0).unwrap();
        register_worker(&conn, "worker-new", "test", 1).unwrap();
        let new = claim_next_for_worker(&conn, "worker-new", 10_000)
            .unwrap()
            .unwrap();
        assert!(checkpoint_owned(
            &conn,
            "f",
            "worker-old",
            old.lease_token.as_deref().unwrap(),
            &serde_json::json!({"old":true}),
            10_000
        )
        .is_err());
        checkpoint_owned(
            &conn,
            "f",
            "worker-new",
            new.lease_token.as_deref().unwrap(),
            &serde_json::json!({"new":true}),
            10_000,
        )
        .unwrap();
        assert_eq!(new.claim_epoch, old.claim_epoch + 1);
        assert_eq!(get(&conn, "f").unwrap().unwrap().recovery_count, 1);
    }
}
