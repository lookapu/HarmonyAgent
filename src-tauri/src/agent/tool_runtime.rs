//! Durable Tool Worker ownership and recovery.
//!
//! Tool execution is deliberately tracked separately from the model/run worker. A process may be
//! alive while one native tool lane is wedged; per-call leases and fencing prevent a late result
//! from overwriting recovery decisions made by another worker.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const DEFAULT_CAPACITY: i64 = 4;
const DEFAULT_LEASE_MS: i64 = 30_000;

#[derive(Clone, Debug, Serialize)]
pub struct ToolLease {
    pub call_id: String,
    pub worker_id: String,
    pub lease_token: String,
    pub attempt: i64,
    pub lease_expires_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolWorkerInfo {
    pub worker_id: String,
    pub process_worker_id: String,
    pub pid: i64,
    pub platform: String,
    pub state: String,
    pub capacity: i64,
    pub active_tools: i64,
    pub started_at: i64,
    pub last_heartbeat_at: i64,
    pub stopped_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ToolRuntimeStats {
    pub active_workers: i64,
    pub lost_workers: i64,
    pub running_tools: i64,
    pub verification_required: i64,
    pub manual_review_required: i64,
    pub recovered_tools: i64,
    pub timed_out_tools: i64,
    pub worker_panics: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn current_worker_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| format!("tool:{}", crate::agent::scheduler::current_worker_id()))
}

/// Stable within a durable call, independent of retries/attempts. Canonical JSON prevents harmless
/// object-key ordering differences from defeating duplicate-side-effect diagnostics.
pub fn idempotency_key(trace_id: &str, call_id: &str, tool: &str, input: &str) -> String {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys = map.keys().collect::<Vec<_>>();
                keys.sort();
                serde_json::Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), canonical(&map[key])))
                        .collect(),
                )
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(canonical).collect())
            }
            value => value.clone(),
        }
    }
    let normalized = serde_json::from_str::<serde_json::Value>(input)
        .map(|value| canonical(&value).to_string())
        .unwrap_or_else(|_| input.trim().to_string());
    let mut digest = Sha256::new();
    digest.update(trace_id.as_bytes());
    digest.update([0]);
    if crate::agent::tools::contracts::contract(tool)
        .effect
        .as_str()
        == "read"
    {
        digest.update(call_id.as_bytes());
        digest.update([0]);
    }
    digest.update(tool.as_bytes());
    digest.update([0]);
    digest.update(normalized.as_bytes());
    format!("tool-v2:{:x}", digest.finalize())
}

pub fn register_current_worker(conn: &Connection) -> Result<(), String> {
    register_worker(conn, current_worker_id(), DEFAULT_CAPACITY)
}

pub fn register_worker(conn: &Connection, worker_id: &str, capacity: i64) -> Result<(), String> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO tool_execution_workers
         (worker_id,process_worker_id,pid,platform,state,capacity,active_tools,started_at,last_heartbeat_at,metadata_json)
         VALUES (?1,?2,?3,?4,'active',?5,0,?6,?6,'{}')
         ON CONFLICT(worker_id) DO UPDATE SET state='active',capacity=excluded.capacity,
         last_heartbeat_at=excluded.last_heartbeat_at,stopped_at=NULL",
        params![
            worker_id,
            crate::agent::scheduler::current_worker_id(),
            i64::from(std::process::id()),
            std::env::consts::OS,
            capacity.max(1),
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn heartbeat_current_worker(conn: &Connection) -> Result<(), String> {
    let now = now_ms();
    conn.execute(
        "UPDATE tool_execution_workers SET last_heartbeat_at=?1,state='active'
         WHERE worker_id=?2 AND state='active'",
        params![now, current_worker_id()],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE tool_runs SET heartbeat_at=?1,lease_expires_at=?2
         WHERE execution_worker_id=?3 AND status IN ('running','verifying')",
        params![now, now + DEFAULT_LEASE_MS, current_worker_id()],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE tool_execution_attempts SET last_heartbeat_at=?1
         WHERE worker_id=?2 AND state='running'",
        params![now, current_worker_id()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn stop_current_worker(conn: &Connection) -> Result<(), String> {
    let now = now_ms();
    conn.execute(
        "UPDATE tool_execution_workers SET state='stopped',stopped_at=?1,last_heartbeat_at=?1,
         active_tools=0 WHERE worker_id=?2",
        params![now, current_worker_id()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Acquires a per-call lease after policy/approval hooks pass. The transaction also serializes the
/// idempotency check, so concurrent app processes cannot start the same side effect twice.
pub fn start_attempt(
    conn: &Connection,
    call_id: &str,
    lease_ms: i64,
) -> Result<Option<ToolLease>, String> {
    register_current_worker(conn)?;
    let now = now_ms();
    let expires = now.saturating_add(lease_ms.max(DEFAULT_LEASE_MS));
    let token = uuid::Uuid::new_v4().to_string();
    let worker = current_worker_id();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let duplicate_effect = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tool_runs current JOIN tool_runs prior
         ON prior.trace_id=current.trace_id AND prior.idempotency_key=current.idempotency_key
         WHERE current.id=?1 AND current.effect_kind!='read' AND prior.id<>current.id
         AND prior.status IN ('running','verifying','ok'))",
            [call_id],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);
    if duplicate_effect {
        tx.execute(
            "UPDATE tool_runs SET status='blocked',verification_state='duplicate_prevented',
             error_code='DUPLICATE_SIDE_EFFECT',result_json='相同副作用调用已执行或正在执行，本次已幂等拦截',
             finished_at=?1,outcome_committed_at=?2 WHERE id=?3 AND status='prepared'",
            params![now / 1000, now, call_id],
        ).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        let _ = crate::agent::enterprise::audit(
            conn,
            None,
            None,
            "tool_worker",
            "tool.duplicate_blocked",
            call_id,
            "blocked",
            &serde_json::json!({"idempotency":"duplicate_side_effect"}),
        );
        return Ok(None);
    }
    let attempt = tx
        .query_row(
            "UPDATE tool_runs SET status='running',execution_worker_id=?1,lease_token=?2,
             attempt=attempt+1,heartbeat_at=?3,lease_expires_at=?4,verification_state='none'
             WHERE id=?5 AND status IN ('prepared','recovery_required')
             RETURNING attempt",
            params![worker, token, now, expires, call_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(attempt) = attempt else {
        return Ok(None);
    };
    tx.execute(
        "INSERT INTO tool_execution_attempts
         (call_id,attempt,worker_id,lease_token,state,started_at,last_heartbeat_at)
         VALUES (?1,?2,?3,?4,'running',?5,?5)",
        params![call_id, attempt, worker, token, now],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE tool_execution_workers SET active_tools=active_tools+1,last_heartbeat_at=?1
         WHERE worker_id=?2",
        params![now, worker],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Some(ToolLease {
        call_id: call_id.into(),
        worker_id: worker.into(),
        lease_token: token,
        attempt,
        lease_expires_at: expires,
    }))
}

pub fn mark_verifying(conn: &Connection, call_id: &str) -> Result<bool, String> {
    let changed = conn.execute(
        "UPDATE tool_runs SET status='verifying',heartbeat_at=?1 WHERE id=?2 AND status='running'
         AND execution_worker_id=?3 AND lease_token IS NOT NULL",
        params![now_ms(), call_id, current_worker_id()],
    ).map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

pub fn start_denial_reason(conn: &Connection, call_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT result_json FROM tool_runs WHERE id=?1 AND status='blocked'",
        [call_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
}

/// Closes the attempt after the caller has durably written the structured outcome under the same
/// owner predicate. Keeping this separate lets the protocol store its complete evidence envelope
/// without duplicating schema knowledge in the worker supervisor.
pub fn close_committed_attempt(
    conn: &Connection,
    call_id: &str,
    status: &str,
    outcome_digest: Option<&str>,
    error: Option<&str>,
) -> Result<bool, String> {
    let now = now_ms();
    let worker = current_worker_id();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let row: Option<(i64, String)> = tx
        .query_row(
            "SELECT attempt,lease_token FROM tool_runs WHERE id=?1 AND execution_worker_id=?2
             AND lease_token IS NOT NULL AND outcome_committed_at IS NOT NULL",
            params![call_id, worker],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((attempt, token)) = row else {
        return Ok(false);
    };
    tx.execute(
        "UPDATE tool_execution_attempts SET state=?1,finished_at=?2,last_heartbeat_at=?2,
         outcome_digest=?3,error=?4 WHERE call_id=?5 AND attempt=?6 AND worker_id=?7
         AND lease_token=?8 AND state='running'",
        params![
            status,
            now,
            outcome_digest,
            error,
            call_id,
            attempt,
            worker,
            token
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE tool_runs SET lease_token=NULL,lease_expires_at=NULL,heartbeat_at=?1
         WHERE id=?2 AND execution_worker_id=?3 AND lease_token=?4",
        params![now, call_id, worker, token],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE tool_execution_workers SET active_tools=MAX(active_tools-1,0),last_heartbeat_at=?1
         WHERE worker_id=?2",
        params![now, worker],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

/// Completes only the currently-owned attempt. A stale process receives `false` and must discard
/// its late result instead of mutating the durable tool outcome.
pub fn finish_owned(
    conn: &Connection,
    call_id: &str,
    status: &str,
    outcome_digest: Option<&str>,
    error: Option<&str>,
) -> Result<bool, String> {
    let now = now_ms();
    let finished_at = now / 1000;
    let worker = current_worker_id();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let row: Option<(i64, String)> = tx
        .query_row(
            "SELECT attempt,lease_token FROM tool_runs
             WHERE id=?1 AND status IN ('running','verifying') AND execution_worker_id=?2",
            params![call_id, worker],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((attempt, token)) = row else {
        return Ok(false);
    };
    let changed = tx
        .execute(
            "UPDATE tool_runs SET status=?1,finished_at=?2,outcome_committed_at=?3,
             heartbeat_at=?3,lease_expires_at=NULL,lease_token=NULL
             WHERE id=?4 AND status IN ('running','verifying') AND execution_worker_id=?5 AND lease_token=?6",
            params![status, finished_at, now, call_id, worker, token],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Ok(false);
    }
    tx.execute(
        "UPDATE tool_execution_attempts SET state=?1,finished_at=?2,last_heartbeat_at=?2,
         outcome_digest=?3,error=?4 WHERE call_id=?5 AND attempt=?6 AND worker_id=?7
         AND lease_token=?8 AND state='running'",
        params![
            status,
            now,
            outcome_digest,
            error,
            call_id,
            attempt,
            worker,
            token
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE tool_execution_workers SET active_tools=MAX(active_tools-1,0),last_heartbeat_at=?1
         WHERE worker_id=?2",
        params![now, worker],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

/// Prepared calls never crossed the side-effect boundary and may terminate without a lease.
pub fn finish_prepared(conn: &Connection, call_id: &str, status: &str) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE tool_runs SET status=?1,finished_at=?2,outcome_committed_at=?3
             WHERE id=?4 AND status='prepared'",
            params![status, now_ms() / 1000, now_ms(), call_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed > 0)
}

/// Recovers only calls whose executor heartbeat and lease have both expired. Read calls can be
/// replayed, write calls require observation, and destructive/manual calls fail closed.
pub fn recover_stale(conn: &Connection, stale_after_ms: i64) -> Result<usize, String> {
    let now = now_ms();
    let finished_at = now / 1000;
    let cutoff = now.saturating_sub(stale_after_ms.max(DEFAULT_LEASE_MS));
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE tool_execution_workers SET state='lost',active_tools=0
         WHERE state='active' AND last_heartbeat_at<?1",
        [cutoff],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE tool_execution_attempts SET state='interrupted',finished_at=?1,
         error='tool worker heartbeat expired' WHERE state='running' AND worker_id IN
         (SELECT worker_id FROM tool_execution_workers WHERE state='lost')",
        [now],
    )
    .map_err(|e| e.to_string())?;
    let changed = tx
        .execute(
            "UPDATE tool_runs SET
               status=CASE recovery_policy WHEN 'replay' THEN 'recovery_required'
                 WHEN 'verify' THEN 'verification_required' ELSE 'manual_review' END,
               verification_state=CASE recovery_policy WHEN 'replay' THEN 'none'
                 WHEN 'verify' THEN 'required' ELSE 'manual' END,
               recovery_count=recovery_count+1,finished_at=?1,lease_expires_at=NULL,lease_token=NULL
             WHERE status IN ('running','verifying') AND lease_expires_at<?2 AND execution_worker_id IN
             (SELECT worker_id FROM tool_execution_workers WHERE state='lost')",
            params![finished_at, now],
        )
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    if changed > 0 {
        let _ = crate::agent::enterprise::audit(
            conn,
            None,
            None,
            "tool_supervisor",
            "tool.worker_recovery",
            "tool_runs",
            "recovered",
            &serde_json::json!({"count":changed,"policy":"effect_aware"}),
        );
    }
    Ok(changed)
}

pub fn runtime_stats(conn: &Connection) -> Result<ToolRuntimeStats, String> {
    conn.query_row(
        "SELECT
          (SELECT COUNT(*) FROM tool_execution_workers WHERE state='active'),
          (SELECT COUNT(*) FROM tool_execution_workers WHERE state='lost'),
          (SELECT COUNT(*) FROM tool_runs WHERE status IN ('running','verifying')),
          (SELECT COUNT(*) FROM tool_runs WHERE status='verification_required'),
          (SELECT COUNT(*) FROM tool_runs WHERE status='manual_review'),
          (SELECT COALESCE(SUM(recovery_count),0) FROM tool_runs),
          (SELECT COUNT(*) FROM tool_runs WHERE error_code='TOOL_TIMEOUT'),
          (SELECT COUNT(*) FROM tool_runs WHERE error_code='TOOL_WORKER_PANIC')",
        [],
        |row| {
            Ok(ToolRuntimeStats {
                active_workers: row.get(0)?,
                lost_workers: row.get(1)?,
                running_tools: row.get(2)?,
                verification_required: row.get(3)?,
                manual_review_required: row.get(4)?,
                recovered_tools: row.get(5)?,
                timed_out_tools: row.get(6)?,
                worker_panics: row.get(7)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

pub fn list_workers(conn: &Connection, limit: usize) -> Result<Vec<ToolWorkerInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT worker_id,process_worker_id,pid,platform,state,capacity,active_tools,
             started_at,last_heartbeat_at,stopped_at FROM tool_execution_workers
             ORDER BY last_heartbeat_at DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit.clamp(1, 500) as i64], |row| {
            Ok(ToolWorkerInfo {
                worker_id: row.get(0)?,
                process_worker_id: row.get(1)?,
                pid: row.get(2)?,
                platform: row.get(3)?,
                state: row.get(4)?,
                capacity: row.get(5)?,
                active_tools: row.get(6)?,
                started_at: row.get(7)?,
                last_heartbeat_at: row.get(8)?,
                stopped_at: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_task_queue(task_id TEXT); CREATE TABLE agent_workers(worker_id TEXT);
             CREATE TABLE tool_runs(id TEXT PRIMARY KEY,status TEXT,recovery_policy TEXT,trace_id TEXT,
               idempotency_key TEXT,effect_kind TEXT,result_json TEXT,
               execution_worker_id TEXT,lease_token TEXT,attempt INTEGER DEFAULT 0,heartbeat_at INTEGER,
               lease_expires_at INTEGER,verification_state TEXT DEFAULT 'none',recovery_count INTEGER DEFAULT 0,
               finished_at INTEGER,outcome_committed_at INTEGER,error_code TEXT);
             CREATE TABLE tool_execution_workers(worker_id TEXT PRIMARY KEY,process_worker_id TEXT,pid INTEGER,
               platform TEXT,state TEXT,capacity INTEGER,active_tools INTEGER,started_at INTEGER,
               last_heartbeat_at INTEGER,stopped_at INTEGER,metadata_json TEXT);
             CREATE TABLE tool_execution_attempts(call_id TEXT,attempt INTEGER,worker_id TEXT,lease_token TEXT,
               state TEXT,started_at INTEGER,last_heartbeat_at INTEGER,finished_at INTEGER,outcome_digest TEXT,
               error TEXT,PRIMARY KEY(call_id,attempt));",
        )
        .unwrap();
        conn
    }

    #[test]
    fn lease_is_fenced_and_terminal_is_immutable() {
        let conn = db();
        conn.execute(
            "INSERT INTO tool_runs(id,status,recovery_policy) VALUES('c','prepared','verify')",
            [],
        )
        .unwrap();
        let lease = start_attempt(&conn, "c", 30_000).unwrap().unwrap();
        assert_eq!(lease.attempt, 1);
        assert!(mark_verifying(&conn, "c").unwrap());
        assert!(finish_owned(&conn, "c", "ok", Some("digest"), None).unwrap());
        assert!(!finish_owned(&conn, "c", "error", None, Some("late")).unwrap());
        let status: String = conn
            .query_row("SELECT status FROM tool_runs WHERE id='c'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "ok");
    }

    #[test]
    fn idempotency_key_canonicalizes_json() {
        assert_eq!(
            idempotency_key("r", "c", "edit_file", r#"{"b":2,"a":1}"#),
            idempotency_key("r", "c", "edit_file", r#"{"a":1,"b":2}"#),
        );
    }

    #[test]
    fn duplicate_side_effect_is_blocked_before_execution() {
        let conn = db();
        let key = idempotency_key("r", "first", "edit_file", r#"{"path":"a"}"#);
        conn.execute("INSERT INTO tool_runs(id,status,recovery_policy,trace_id,idempotency_key,effect_kind)
          VALUES('first','prepared','verify','r',?1,'write'),('second','prepared','verify','r',?1,'write')", [&key]).unwrap();
        start_attempt(&conn, "first", 30_000).unwrap().unwrap();
        assert!(start_attempt(&conn, "second", 30_000).unwrap().is_none());
        let state: String = conn
            .query_row(
                "SELECT verification_state FROM tool_runs WHERE id='second'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "duplicate_prevented");
    }

    #[test]
    fn stale_write_requires_verification_but_read_can_replay() {
        let conn = db();
        register_current_worker(&conn).unwrap();
        conn.execute("INSERT INTO tool_runs(id,status,recovery_policy) VALUES('read','prepared','replay'),('write','prepared','verify'),('push','prepared','manual')", []).unwrap();
        for id in ["read", "write", "push"] {
            start_attempt(&conn, id, 30_000).unwrap();
        }
        conn.execute("UPDATE tool_execution_workers SET last_heartbeat_at=0", [])
            .unwrap();
        conn.execute("UPDATE tool_runs SET lease_expires_at=0", [])
            .unwrap();
        assert_eq!(recover_stale(&conn, 30_000).unwrap(), 3);
        let states: Vec<String> = ["read", "write", "push"]
            .iter()
            .map(|id| {
                conn.query_row("SELECT status FROM tool_runs WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .unwrap()
            })
            .collect();
        assert_eq!(
            states,
            [
                "recovery_required",
                "verification_required",
                "manual_review"
            ]
        );
    }
}
