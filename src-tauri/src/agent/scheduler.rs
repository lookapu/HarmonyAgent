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
    let task_id = format!("task:{run_id}");
    let now = now_ms();
    conn.execute(
        "INSERT INTO agent_task_queue
         (task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
          lease_expires_at,budget_json,checkpoint_json,created_at,updated_at)
         VALUES (?1,?2,?3,?4,'running',?5,?6,1,3,?7,?8,'{}',?9,?9)
         ON CONFLICT(run_id) DO UPDATE SET state='running',worker_id=excluded.worker_id,
          lease_expires_at=excluded.lease_expires_at,budget_json=excluded.budget_json,updated_at=excluded.updated_at",
        params![task_id, run_id, conversation_id, goal, priority.clamp(0, 100), worker_id(),
            now.saturating_add(lease_ms.max(10_000)), budget.to_string(), now],
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
        "UPDATE agent_task_queue SET checkpoint_json=?1,lease_expires_at=?2,updated_at=?3
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
                lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at
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
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn worker_id() -> String {
    format!("desktop:{}", std::process::id())
}

#[cfg(test)]
pub fn get(conn: &Connection, run_id: &str) -> Result<Option<ScheduledTask>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT task_id,run_id,conversation_id,goal,state,priority,worker_id,attempt,max_attempts,
                lease_expires_at,budget_json,checkpoint_json,error,created_at,updated_at,finished_at
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
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY,scheduler_task_id TEXT,budget_json TEXT); CREATE TABLE conversations(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c'); CREATE TABLE agent_task_queue(task_id TEXT PRIMARY KEY,run_id TEXT UNIQUE,conversation_id TEXT,goal TEXT,state TEXT,priority INTEGER,worker_id TEXT,attempt INTEGER,max_attempts INTEGER,lease_expires_at INTEGER,budget_json TEXT,checkpoint_json TEXT,error TEXT,created_at INTEGER,updated_at INTEGER,finished_at INTEGER);").unwrap();
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
}
