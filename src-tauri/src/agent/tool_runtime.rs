//! Durable Tool Worker ownership and recovery.
//!
//! Tool execution is deliberately tracked separately from the model/run worker. A process may be
//! alive while one native tool lane is wedged; per-call leases and fencing prevent a late result
//! from overwriting recovery decisions made by another worker.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
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
    pub thread_id: Option<i64>,
    pub thread_name: Option<String>,
    pub stuck_count: i64,
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
    pub stuck_tools: i64,
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

/// Marks calls stuck: the caller already gave up (timeout/cancel) but the execution thread may
/// still be running. Attribution only — recovery still follows `recover_stale` by effect type.
pub fn mark_stuck(conn: &Connection, call_id: &str) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE tool_runs SET verification_state='stuck_detected'
             WHERE id=?1 AND status IN ('running','verifying')
             AND verification_state NOT IN ('stuck_detected','required','manual')",
            [call_id],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE tool_execution_workers SET stuck_count=stuck_count+1
         WHERE worker_id=?1 AND state='active'",
        [current_worker_id()],
    )
    .map_err(|e| e.to_string())?;
    let _ = crate::agent::enterprise::audit(
        conn,
        None,
        None,
        "tool_supervisor",
        "tool.worker_stuck",
        "tool_runs",
        "stuck_detected",
        &serde_json::json!({"call_id": call_id}),
    );
    Ok(true)
}

/// Background stuck scan: leases expired without the caller marking them (e.g. the awaiting task
/// crashed first). Attribution only; run before `recover_stale` so stuck vs recovered stays
/// distinguishable in the control plane.
pub fn detect_stuck(conn: &Connection, stale_after_ms: i64) -> Result<usize, String> {
    let now = now_ms();
    let cutoff = now.saturating_sub(stale_after_ms.max(DEFAULT_LEASE_MS));
    let changed = conn
        .execute(
            "UPDATE tool_runs SET verification_state='stuck_detected'
             WHERE status IN ('running','verifying') AND lease_expires_at<?1
             AND verification_state NOT IN ('stuck_detected','required','manual')",
            [cutoff],
        )
        .map_err(|e| e.to_string())?;
    Ok(changed)
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
          (SELECT COUNT(*) FROM tool_runs WHERE error_code='TOOL_WORKER_PANIC'),
          (SELECT COALESCE(SUM(stuck_count),0) FROM tool_execution_workers)",
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
                stuck_tools: row.get(8)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

pub fn list_workers(conn: &Connection, limit: usize) -> Result<Vec<ToolWorkerInfo>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT worker_id,process_worker_id,pid,platform,state,capacity,active_tools,
             started_at,last_heartbeat_at,stopped_at,thread_id,thread_name,stuck_count
             FROM tool_execution_workers ORDER BY last_heartbeat_at DESC LIMIT ?1",
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
                thread_id: row.get(10)?,
                thread_name: row.get(11)?,
                stuck_count: row.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

// ---------------- 专用工具执行线程（Execution Lane） ----------------

static NEXT_EXECUTION_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// 专用工具执行线程的句柄：run_tool 在独立 OS 线程中 block_on 执行，与调用方
/// tokio 任务、UI 线程和共享任务池隔离；panic 在线程内被捕获并转为错误返回，
/// 线程死亡不会拖垮主进程。调用方超时/取消放弃后线程可能仍在运行，由
/// `mark_stuck` 归因（卡顿指标），恢复语义不变（recover_stale 按 effect 分类）。
pub struct ExecutionLane {
    thread_name: String,
    handle: Option<std::thread::JoinHandle<()>>,
    pub(crate) result: tokio::sync::oneshot::Receiver<Result<String, String>>,
}

impl ExecutionLane {
    /// 执行线程是否已结束（panic 或正常完成）。
    pub fn is_finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(|handle| handle.is_finished())
    }

    /// 等待执行结果；超时返回 Err（调用方应放弃并标记卡死）。
    pub async fn await_result(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<String, String> {
        match tokio::time::timeout(timeout, &mut self.result).await {
            Ok(Ok(Ok(out))) => Ok(out),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(format!(
                "工具执行线程终止但未返回结果：{}",
                self.thread_name
            )),
            Err(_) => Err(format!("工具执行超时：{}", self.thread_name)),
        }
    }
}

/// 在专用 OS 线程中执行工具 future。线程启动后把真实线程身份登记到
/// tool_execution_workers（thread_id/thread_name），使控制面可见真实执行单元。
/// `db` 供线程内登记身份用（无 State 上下文时走全局 DB 单例）。
/// 必须在 tokio 运行时上下文中调用（内部取 Handle 供线程 block_on）。
pub fn spawn_execution<F>(
    _call_id: &str,
    tool: &str,
    fut: F,
    db: Option<std::sync::Arc<std::sync::Mutex<Connection>>>,
) -> ExecutionLane
where
    F: std::future::Future<Output = Result<String, String>> + Send + 'static,
{
    let rt = tokio::runtime::Handle::current();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let thread_name = format!("tool-exec:{tool}");
    let thread_name_clone = thread_name.clone();
    let thread_name_owned = thread_name.clone();
    let tool_owned = tool.to_string();
    let db = db.or_else(crate::db::global);
    let handle = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            // 登记真实线程身份（供控制面观测与卡死归因）
            if let Some(conn) = db {
                if let Ok(conn) = conn.lock() {
                    let tid = NEXT_EXECUTION_THREAD_ID.fetch_add(1, Ordering::Relaxed);
                    let _ = conn.execute(
                        "UPDATE tool_execution_workers SET thread_id=?1,thread_name=?2,
                         state='active',last_heartbeat_at=?3 WHERE worker_id=?4",
                        params![tid as i64, thread_name_clone, now_ms(), current_worker_id()],
                    );
                }
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rt.block_on(fut)
            }))
            .unwrap_or_else(|_| {
                Err(format!("工具执行器发生 panic，已隔离当前调用：{tool_owned}"))
            });
            let _ = tx.send(result);
        })
        .expect("spawn tool execution thread");
    ExecutionLane {
        thread_name: thread_name_owned,
        handle: Some(handle),
        result: rx,
    }
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
               last_heartbeat_at INTEGER,stopped_at INTEGER,metadata_json TEXT,thread_id INTEGER,
               thread_name TEXT,stuck_count INTEGER NOT NULL DEFAULT 0);
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

    #[test]
    fn execution_thread_registers_real_identity() {
        let db_arc = std::sync::Arc::new(std::sync::Mutex::new(db()));
        register_current_worker(&db_arc.lock().unwrap()).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut lane = spawn_execution(
                "c",
                "read_file",
                async { Ok("ok".to_string()) },
                Some(db_arc.clone()),
            );
            assert_eq!(
                lane.await_result(std::time::Duration::from_secs(5))
                    .await
                    .unwrap(),
                "ok"
            );
        });
        let conn = db_arc.lock().unwrap();
        let (tid, tname): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT thread_id,thread_name FROM tool_execution_workers WHERE worker_id=?1",
                [current_worker_id()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(tid.is_some() && tid.unwrap() > 0);
        assert!(tname.as_deref().unwrap_or("").contains("tool-exec:read_file"));
    }

    #[test]
    fn hung_execution_lane_times_out_without_blocking_caller() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut lane = spawn_execution(
                "hung",
                "run_command",
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    Ok("eventually-finished".to_string())
                },
                None,
            );
            let started = std::time::Instant::now();
            let err = lane
                .await_result(std::time::Duration::from_millis(15))
                .await
                .unwrap_err();
            assert!(err.contains("超时"), "{err}");
            assert!(
                started.elapsed() < std::time::Duration::from_secs(1),
                "卡死执行线程不应阻塞调用方"
            );
            assert!(!lane.is_finished(), "超时时执行线程仍应被隔离在后台");
            assert_eq!(
                lane.await_result(std::time::Duration::from_secs(2))
                    .await
                    .unwrap(),
                "eventually-finished"
            );
        });
    }

    #[test]
    fn uncancellable_late_result_is_fenced_after_recovery() {
        let db_arc = std::sync::Arc::new(std::sync::Mutex::new(db()));
        register_current_worker(&db_arc.lock().unwrap()).unwrap();
        db_arc
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO tool_runs(id,status,recovery_policy,trace_id,idempotency_key,effect_kind)
                 VALUES('late','prepared','verify','r','late-key','write')",
                [],
            )
            .unwrap();
        start_attempt(&db_arc.lock().unwrap(), "late", 30_000)
            .unwrap()
            .unwrap();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut lane = spawn_execution(
                "late",
                "edit_file",
                async {
                    // 模拟忽略协作式取消、最终仍返回结果的第三方/阻塞工具。
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    Ok("late-side-effect-result".to_string())
                },
                Some(db_arc.clone()),
            );
            assert!(lane
                .await_result(std::time::Duration::from_millis(15))
                .await
                .unwrap_err()
                .contains("超时"));
            assert!(mark_stuck(&db_arc.lock().unwrap(), "late").unwrap());
            db_arc
                .lock()
                .unwrap()
                .execute("UPDATE tool_execution_workers SET last_heartbeat_at=0", [])
                .unwrap();
            db_arc
                .lock()
                .unwrap()
                .execute("UPDATE tool_runs SET lease_expires_at=0", [])
                .unwrap();
            assert_eq!(recover_stale(&db_arc.lock().unwrap(), 30_000).unwrap(), 1);

            assert_eq!(
                lane.await_result(std::time::Duration::from_secs(2))
                    .await
                    .unwrap(),
                "late-side-effect-result"
            );
            assert!(
                !finish_owned(
                    &db_arc.lock().unwrap(),
                    "late",
                    "ok",
                    Some("late-digest"),
                    None
                )
                .unwrap(),
                "恢复完成后，失去租约的迟到结果不得提交"
            );
        });
        let conn = db_arc.lock().unwrap();
        let (status, verification): (String, String) = conn
            .query_row(
                "SELECT status,verification_state FROM tool_runs WHERE id='late'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "verification_required");
        assert_eq!(verification, "required");
    }

    #[test]
    fn panicked_execution_thread_is_isolated_and_reported() {
        let db_arc = std::sync::Arc::new(std::sync::Mutex::new(db()));
        register_current_worker(&db_arc.lock().unwrap()).unwrap();
        db_arc
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO tool_runs(id,status,recovery_policy,trace_id,idempotency_key,effect_kind)
                 VALUES('p','prepared','verify','r','k','write')",
                [],
            )
            .unwrap();
        let lease = start_attempt(&db_arc.lock().unwrap(), "p", 30_000)
            .unwrap()
            .unwrap();
        assert_eq!(lease.attempt, 1);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut lane = rt.block_on(async {
            let lane = spawn_execution(
                "p",
                "edit_file",
                async {
                    panic!("boom");
                },
                Some(db_arc.clone()),
            );
            lane
        });
        let err = rt
            .block_on(lane.await_result(std::time::Duration::from_secs(5)))
            .unwrap_err();
        assert!(err.contains("panic"), "{err}");
        // panic 被隔离：调用仍处 running，由恢复协议接管
        assert!(mark_stuck(&db_arc.lock().unwrap(), "p").unwrap());
        let state: String = db_arc
            .lock()
            .unwrap()
            .query_row(
                "SELECT verification_state FROM tool_runs WHERE id='p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "stuck_detected");
        assert_eq!(runtime_stats(&db_arc.lock().unwrap()).unwrap().stuck_tools, 1);
        // 心跳过期后按 effect 分类恢复
        db_arc
            .lock()
            .unwrap()
            .execute("UPDATE tool_execution_workers SET last_heartbeat_at=0", [])
            .unwrap();
        db_arc
            .lock()
            .unwrap()
            .execute("UPDATE tool_runs SET lease_expires_at=0", [])
            .unwrap();
        assert_eq!(recover_stale(&db_arc.lock().unwrap(), 30_000).unwrap(), 1);
        let status: String = db_arc
            .lock()
            .unwrap()
            .query_row("SELECT status FROM tool_runs WHERE id='p'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(status, "verification_required");
    }

    #[test]
    fn stale_scan_marks_stuck_without_recovery() {
        let conn = db();
        register_current_worker(&conn).unwrap();
        conn.execute(
            "INSERT INTO tool_runs(id,status,recovery_policy)
             VALUES('s','prepared','verify')",
            [],
        )
        .unwrap();
        start_attempt(&conn, "s", 30_000).unwrap();
        conn.execute("UPDATE tool_runs SET lease_expires_at=0", [])
            .unwrap();
        assert_eq!(detect_stuck(&conn, 30_000).unwrap(), 1);
        let state: String = conn
            .query_row(
                "SELECT verification_state FROM tool_runs WHERE id='s'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "stuck_detected");
        // 标记后不重复累计
        assert_eq!(detect_stuck(&conn, 30_000).unwrap(), 0);
    }
}
