//! SQLite 持久化任务调度器：进程内 Future 负责执行，数据库队列负责所有权、优先级、
//! 租约、检查点与重启后的安全接管。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

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
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let task_id = tx
        .query_row(
            "SELECT q.task_id FROM agent_task_queue q
         WHERE q.state IN ('queued','recovery_required')
           AND (q.next_attempt_at IS NULL OR q.next_attempt_at<=?1)
           AND q.attempt<q.max_attempts
           AND (q.concurrency_key IS NULL OR NOT EXISTS(
             SELECT 1 FROM agent_task_queue active WHERE active.concurrency_key=q.concurrency_key
             AND active.state='running' AND active.task_id!=q.task_id))
         ORDER BY q.priority DESC,q.created_at ASC LIMIT 1",
            [now],
            |row| row.get::<_, String>(0),
        )
        .ok();
    let Some(task_id) = task_id else {
        tx.commit().map_err(|e| e.to_string())?;
        return Ok(None);
    };
    let changed = tx
        .execute(
            "UPDATE agent_task_queue SET state='running',worker_id=?1,attempt=attempt+1,
         claimed_at=?2,lease_expires_at=?3,next_attempt_at=NULL,error=NULL,updated_at=?2
         WHERE task_id=?4 AND state IN ('queued','recovery_required')",
            params![
                worker_id(),
                now,
                now.saturating_add(lease_ms.max(10_000)),
                task_id
            ],
        )
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    if changed == 0 {
        return Ok(None);
    }
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
    conn.execute(
        "UPDATE agent_task_queue SET state='running',worker_id=?1,attempt=CASE WHEN attempt=0 THEN 1 ELSE attempt END,
         claimed_at=COALESCE(claimed_at,?2),lease_expires_at=?3,updated_at=?2 WHERE task_id=?4",
        params![worker_id(),now,now.saturating_add(lease_ms.max(10_000)),task_id],
    ).map_err(|e| e.to_string())?;
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
    let now = now_ms();
    conn.execute(
        "UPDATE agent_task_queue SET checkpoint_json=?1,lease_expires_at=?2,last_checkpoint_at=?3,updated_at=?3
         WHERE run_id=?4 AND state='running'",
        params![
            checkpoint.to_string(),
            now.saturating_add(lease_ms.max(10_000)),
            now,
            run_id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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
         worker_id=NULL,lease_expires_at=NULL,next_attempt_at=CASE WHEN attempt>=max_attempts THEN NULL ELSE ?1 END,
         error=?2,updated_at=?3,finished_at=CASE WHEN attempt>=max_attempts THEN ?3 ELSE NULL END
         WHERE run_id=?4 AND state='running'",
        params![now.saturating_add(delay_ms.max(0)),error,now,run_id],
    ).map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

pub fn request_resume(conn: &Connection, run_id: &str, resume_token: &str) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE agent_task_queue SET state='queued',worker_id=NULL,lease_expires_at=NULL,
         next_attempt_at=?1,error=NULL,updated_at=?1 WHERE run_id=?2 AND resume_token=?3
         AND state='recovery_required' AND attempt<max_attempts",
            params![now_ms(), run_id, resume_token],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

pub fn mark_recovery_required(conn: &Connection, run_id: &str, error: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_task_queue SET state='recovery_required',worker_id=NULL,lease_expires_at=NULL,
         next_attempt_at=NULL,error=?1,updated_at=?2 WHERE run_id=?3
         AND state NOT IN ('completed','cancelled')",
        params![error,now_ms(),run_id],
    ).map_err(|e| e.to_string())?;
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
    conn.execute(
        "UPDATE agent_task_queue SET state=?1,error=?2,lease_expires_at=NULL,updated_at=?3,finished_at=?3
         WHERE run_id=?4 AND state NOT IN ('completed','failed','cancelled')",
        params![state, error, now, run_id],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// 仅回收真正过期的执行所有权；任务是否重放由 Recovery Orchestrator 决定。
pub fn recover_expired(conn: &Connection) -> Result<usize, String> {
    let now = now_ms();
    conn.execute(
        "UPDATE agent_task_queue SET state='recovery_required',worker_id=NULL,
         error=COALESCE(error,'任务租约过期，等待安全恢复'),updated_at=?1
         WHERE state='running' AND lease_expires_at IS NOT NULL AND lease_expires_at<?1",
        [now],
    )
    .map_err(|e| e.to_string())
}

pub fn recover_orphaned_on_startup(conn: &Connection) -> Result<usize, String> {
    conn.execute(
        "UPDATE agent_task_queue SET state='recovery_required',worker_id=NULL,lease_expires_at=NULL,
         error=COALESCE(error,'应用重启，等待 Recovery Orchestrator 安全接管'),updated_at=?1
         WHERE state='running'",
        [now_ms()],
    ).map_err(|e| e.to_string())
}

pub fn list(conn: &Connection, limit: usize) -> Result<Vec<ScheduledTask>, String> {
    let mut stmt = conn.prepare(
        "SELECT task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
                lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at,
                payload_json,resume_token,claimed_at,last_checkpoint_at,next_attempt_at,concurrency_key,tenant_id
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
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn worker_id() -> String {
    format!("desktop:{}", std::process::id())
}

pub fn get(conn: &Connection, run_id: &str) -> Result<Option<ScheduledTask>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
                lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at,
                payload_json,resume_token,claimed_at,last_checkpoint_at,next_attempt_at,concurrency_key,tenant_id
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
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn get_by_task_id(conn: &Connection, task_id: &str) -> Result<Option<ScheduledTask>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
         lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at,
         payload_json,resume_token,claimed_at,last_checkpoint_at,next_attempt_at,concurrency_key,tenant_id
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY,scheduler_task_id TEXT,budget_json TEXT); CREATE TABLE conversations(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c'); CREATE TABLE agent_task_queue(task_id TEXT PRIMARY KEY,run_id TEXT UNIQUE,conversation_id TEXT,goal TEXT,state TEXT,priority INTEGER,worker_id TEXT,attempt INTEGER,max_attempts INTEGER,lease_expires_at INTEGER,budget_json TEXT,checkpoint_json TEXT,error TEXT,created_at INTEGER,updated_at INTEGER,finished_at INTEGER,payload_json TEXT NOT NULL DEFAULT '{}',resume_token TEXT,claimed_at INTEGER,last_checkpoint_at INTEGER,next_attempt_at INTEGER,concurrency_key TEXT,tenant_id TEXT NOT NULL DEFAULT 'local');").unwrap();
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
}
