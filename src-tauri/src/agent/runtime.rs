//! Durable Agent Runtime。
//!
//! 任务运行状态与事件游标落 SQLite，保证 WebView 重载、应用崩溃和系统重启后，
//! UI 能判断任务的真实终态并从最后事件序号继续补拉，而不是依赖进程内布尔值猜测。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentRun {
    pub run_id: String,
    pub conversation_id: String,
    pub goal: String,
    pub state: String,
    pub phase: String,
    pub attempt: i64,
    pub last_event_seq: i64,
    pub recovery_count: i64,
    pub resume_policy: String,
    pub parent_run_id: Option<String>,
    pub recovery_plan_json: Option<String>,
    pub recovery_mode: String,
    pub acceptance_json: Option<String>,
    pub goal_contract_json: Option<String>,
    pub remediation_count: i64,
    pub heartbeat_at: Option<i64>,
    pub lease_expires_at: Option<i64>,
    pub quality_json: Option<String>,
    pub error: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunEvent {
    pub event_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub seq: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
pub const DEFAULT_LEASE_MS: i64 = 120_000;

#[cfg(test)]
pub fn begin_run(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    goal: &str,
) -> Result<(), String> {
    begin_run_with_recovery(conn, run_id, conversation_id, goal, None)
}

#[cfg(test)]
pub fn begin_run_with_recovery(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    goal: &str,
    recovery: Option<&crate::agent::recovery::RecoveryPlan>,
) -> Result<(), String> {
    begin_managed_run(conn, run_id, conversation_id, goal, recovery, None, DEFAULT_LEASE_MS)
}

pub fn begin_managed_run(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    goal: &str,
    recovery: Option<&crate::agent::recovery::RecoveryPlan>,
    contract: Option<&crate::agent::acceptance::GoalContract>,
    lease_ms: i64,
) -> Result<(), String> {
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    // 进程异常退出遗留的活跃任务先明确收敛。正常并发仍由唯一索引拒绝。
    tx.execute(
        "UPDATE agent_runs SET state='interrupted', phase='recovery_required',
         error=COALESCE(error, '新的任务启动前发现遗留的非终态运行'),
         recovery_count=recovery_count+1, updated_at=?1, finished_at=?1
         WHERE conversation_id=?2
           AND state IN ('queued','running','waiting_approval','waiting_user','verifying')",
        params![now, conversation_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO agent_runs
         (run_id,conversation_id,goal,state,phase,attempt,last_event_seq,recovery_count,
          resume_policy,metadata_json,started_at,updated_at,parent_run_id,recovery_plan_json,recovery_mode,
          goal_contract_json,heartbeat_at,lease_expires_at)
         VALUES (?1,?2,?3,'running',?4,
                 COALESCE((SELECT attempt+1 FROM agent_runs WHERE run_id=?7),1),
                 0,0,?5,'{}',?6,?6,?7,?8,?9,?10,?6,?11)",
        params![
            run_id,
            conversation_id,
            goal,
            if recovery.is_some() { "recovering" } else { "initializing" },
            recovery.map(|r| r.policy.as_str()).unwrap_or("continue"),
            now,
            recovery.map(|r| r.parent_run_id.as_str()),
            recovery.and_then(|r| serde_json::to_string(r).ok()),
            if recovery.is_some() { "resume" } else { "fresh" },
            contract.and_then(|value| serde_json::to_string(value).ok()),
            now.saturating_add(lease_ms.max(10_000)),
        ],
    )
    .map_err(|e| e.to_string())?;
    append_event_tx(
        &tx,
        run_id,
        conversation_id,
        "run.started",
        &serde_json::json!({
            "goal": goal,
            "state": "running",
            "phase": if recovery.is_some() { "recovering" } else { "initializing" },
            "parent_run_id": recovery.map(|r| r.parent_run_id.as_str()),
            "goal_contract_version": contract.map(|value| value.version),
            "lease_expires_at": now.saturating_add(lease_ms.max(10_000)),
        }),
        now,
    )?;
    if let Some(plan) = recovery {
        append_event_tx(
            &tx,
            run_id,
            conversation_id,
            "recovery.planned",
            &serde_json::to_value(plan).unwrap_or_default(),
            now,
        )?;
    }
    tx.commit().map_err(|e| e.to_string())
}

fn append_event_tx(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
    now: i64,
) -> Result<i64, String> {
    let seq: i64 = conn
        .query_row(
            "UPDATE agent_runs SET last_event_seq=last_event_seq+1, updated_at=?1
             WHERE run_id=?2 RETURNING last_event_seq",
            params![now, run_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO run_events(event_id,run_id,conversation_id,seq,event_type,payload,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            uuid::Uuid::new_v4().to_string(),
            run_id,
            conversation_id,
            seq,
            event_type,
            payload.to_string(),
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(seq)
}

pub fn append_event(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<i64, String> {
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let seq = append_event_tx(&tx, run_id, conversation_id, event_type, &payload, now)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(seq)
}

pub fn transition(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    state: &str,
    phase: &str,
    error: Option<&str>,
) -> Result<i64, String> {
    // 终态不可逆：迟到的 watchdog、Drop 或 IPC 收尾不得把已完成任务重新标成中断，
    // 也不得把已取消任务改写成成功。SQLite 连接锁保证检查与更新不会并发穿透。
    let current: Option<(String, i64, String, Option<String>)> = conn
        .query_row(
            "SELECT state,last_event_seq,recovery_mode,parent_run_id FROM agent_runs WHERE run_id=?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((current_state, current_seq, recovery_mode, parent_run_id)) = current else {
        return Err(format!("运行不存在：{run_id}"));
    };
    if matches!(
        current_state.as_str(),
        "completed" | "failed" | "cancelled" | "interrupted"
    ) {
        return Ok(current_seq);
    }
    let terminal = matches!(state, "completed" | "failed" | "cancelled" | "interrupted");
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let changed = tx
        .execute(
            "UPDATE agent_runs SET state=?1, phase=?2, error=?3, updated_at=?4,
             finished_at=CASE WHEN ?5=1 THEN ?4 ELSE NULL END,
             lease_expires_at=CASE WHEN ?5=1 THEN NULL ELSE lease_expires_at END WHERE run_id=?6",
            params![
                state,
                phase,
                error,
                now,
                if terminal { 1 } else { 0 },
                run_id
            ],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(format!("运行不存在：{run_id}"));
    }
    let seq = append_event_tx(
        &tx,
        run_id,
        conversation_id,
        "run.transition",
        &serde_json::json!({ "state": state, "phase": phase, "error": error }),
        now,
    )?;
    if terminal && recovery_mode == "resume" {
        append_event_tx(
            &tx,
            run_id,
            conversation_id,
            if state == "completed" {
                "recovery.completed"
            } else {
                "recovery.terminated"
            },
            &serde_json::json!({
                "parent_run_id": parent_run_id,
                "state": state,
                "phase": phase,
                "error": error,
            }),
            now,
        )?;
    }
    // 模型 delta 仅服务运行中断后的补拉；正常完成的正文已经进入 messages，重复的
    // checkpoint 可安全删除。失败/取消/中断仍保留，以便诊断和恢复部分输出。
    if state == "completed" {
        tx.execute(
            "DELETE FROM run_events WHERE run_id=?1 AND event_type='model.delta'",
            [run_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(seq)
}

/// 无 Tauri State 的收尾路径（RAII Drop / 独立看门狗线程）使用全局连接收敛状态。
pub fn transition_global(
    run_id: &str,
    conversation_id: &str,
    state: &str,
    phase: &str,
    error: Option<&str>,
) {
    let Some(db) = crate::db::global() else {
        return;
    };
    let Ok(conn) = db.lock() else { return };
    let _ = transition(&conn, run_id, conversation_id, state, phase, error);
}

pub fn set_acceptance(
    conn: &Connection,
    run_id: &str,
    acceptance: &serde_json::Value,
) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_runs SET acceptance_json=?1, updated_at=?2 WHERE run_id=?3",
        params![acceptance.to_string(), now_ms(), run_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn renew_lease(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    phase: &str,
    lease_ms: i64,
) -> Result<i64, String> {
    let now = now_ms();
    let expires = now.saturating_add(lease_ms.max(10_000));
    conn.execute(
        "UPDATE agent_runs SET heartbeat_at=?1,lease_expires_at=?2,phase=?3,updated_at=?1
         WHERE run_id=?4 AND state IN ('queued','running','waiting_approval','waiting_user','verifying')",
        params![now, expires, phase, run_id],
    ).map_err(|e| e.to_string())?;
    append_event(conn, run_id, conversation_id, "run.heartbeat", serde_json::json!({
        "phase": phase, "lease_expires_at": expires,
    }))
}

/// 高频看门狗心跳只续租，不追加事件，避免长任务每 5 秒膨胀事件表。
pub fn touch_lease_global(run_id: &str, phase: &str, lease_ms: i64) {
    let Some(db) = crate::db::global() else { return };
    let Ok(conn) = db.lock() else { return };
    let now = now_ms();
    let _ = conn.execute(
        "UPDATE agent_runs SET heartbeat_at=?1,lease_expires_at=?2,phase=?3,updated_at=?1
         WHERE run_id=?4 AND state IN ('queued','running','waiting_approval','waiting_user','verifying')",
        params![now, now.saturating_add(lease_ms.max(10_000)), phase, run_id],
    );
}

pub fn record_remediation(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    blockers: &[String],
) -> Result<i64, String> {
    conn.execute(
        "UPDATE agent_runs SET remediation_count=remediation_count+1,phase='remediating',updated_at=?1 WHERE run_id=?2",
        params![now_ms(), run_id],
    ).map_err(|e| e.to_string())?;
    append_event(conn, run_id, conversation_id, "acceptance.remediation_requested", serde_json::json!({ "blockers": blockers }))
}

pub fn set_quality(conn: &Connection, run_id: &str, quality: &serde_json::Value) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_runs SET quality_json=?1,updated_at=?2 WHERE run_id=?3",
        params![quality.to_string(), now_ms(), run_id],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<AgentRun>, String> {
    conn.query_row(
        "SELECT run_id,conversation_id,goal,state,phase,attempt,last_event_seq,recovery_count,
                resume_policy,acceptance_json,error,started_at,updated_at,finished_at,
                parent_run_id,recovery_plan_json,recovery_mode,goal_contract_json,
                remediation_count,heartbeat_at,lease_expires_at,quality_json
         FROM agent_runs WHERE run_id=?1",
        [run_id],
        |r| {
            Ok(AgentRun {
                run_id: r.get(0)?,
                conversation_id: r.get(1)?,
                goal: r.get(2)?,
                state: r.get(3)?,
                phase: r.get(4)?,
                attempt: r.get(5)?,
                last_event_seq: r.get(6)?,
                recovery_count: r.get(7)?,
                resume_policy: r.get(8)?,
                acceptance_json: r.get(9)?,
                error: r.get(10)?,
                started_at: r.get(11)?,
                updated_at: r.get(12)?,
                finished_at: r.get(13)?,
                parent_run_id: r.get(14)?,
                recovery_plan_json: r.get(15)?,
                recovery_mode: r.get(16)?,
                goal_contract_json: r.get(17)?,
                remediation_count: r.get(18)?,
                heartbeat_at: r.get(19)?,
                lease_expires_at: r.get(20)?,
                quality_json: r.get(21)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn latest_run(conn: &Connection, conversation_id: &str) -> Result<Option<AgentRun>, String> {
    let run_id: Option<String> = conn
        .query_row(
            "SELECT run_id FROM agent_runs WHERE conversation_id=?1 ORDER BY started_at DESC LIMIT 1",
            [conversation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match run_id {
        Some(id) => get_run(conn, &id),
        None => Ok(None),
    }
}

pub fn events_after(
    conn: &Connection,
    run_id: &str,
    after_seq: i64,
    limit: usize,
) -> Result<Vec<RunEvent>, String> {
    let limit = limit.clamp(1, 1000) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT event_id,run_id,conversation_id,seq,event_type,payload,created_at
             FROM run_events WHERE run_id=?1 AND seq>?2 ORDER BY seq ASC LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![run_id, after_seq, limit], |r| {
            let raw: String = r.get(5)?;
            Ok(RunEvent {
                event_id: r.get(0)?,
                run_id: r.get(1)?,
                conversation_id: r.get(2)?,
                seq: r.get(3)?,
                event_type: r.get(4)?,
                payload: serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null),
                created_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 启动恢复：进程不可能继续持有上次的 Future，因此所有非终态任务必须明确标为 interrupted。
/// UI 可基于 resume_policy/phase 提供“继续任务”，禁止显示为仍在运行。
pub fn recover_interrupted_runs(conn: &Connection) -> Result<usize, String> {
    let now = now_ms();
    let ids = {
        let mut stmt = conn
            .prepare(
                "SELECT run_id,conversation_id FROM agent_runs
                 WHERE state IN ('queued','running','waiting_approval','waiting_user','verifying')",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    for (run_id, conversation_id) in &ids {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE agent_runs SET state='interrupted',phase='recovery_required',
             recovery_count=recovery_count+1,error='应用退出导致任务中断，可从已持久化进度继续',
             resume_policy=CASE
               WHEN EXISTS(SELECT 1 FROM execution_steps WHERE run_id=?2 AND state='interrupted' AND recovery_policy='manual')
                 OR EXISTS(SELECT 1 FROM tool_runs WHERE trace_id=?2 AND status='interrupted' AND recovery_policy='manual') THEN 'manual'
               WHEN EXISTS(SELECT 1 FROM execution_steps WHERE run_id=?2 AND state='interrupted' AND recovery_policy='verify')
                 OR EXISTS(SELECT 1 FROM tool_runs WHERE trace_id=?2 AND status='interrupted' AND recovery_policy='verify') THEN 'verify_effects'
               ELSE 'continue'
             END,
             updated_at=?1,finished_at=?1 WHERE run_id=?2",
            params![now, run_id],
        )
        .map_err(|e| e.to_string())?;
        append_event_tx(
            &tx,
            run_id,
            conversation_id,
            "run.recovered",
            &serde_json::json!({ "state": "interrupted", "phase": "recovery_required" }),
            now,
        )?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE conversations(id TEXT PRIMARY KEY);
             INSERT INTO conversations(id) VALUES ('c');
             CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id),goal TEXT NOT NULL DEFAULT '',state TEXT NOT NULL,phase TEXT NOT NULL,attempt INTEGER NOT NULL DEFAULT 1,last_event_seq INTEGER NOT NULL DEFAULT 0,recovery_count INTEGER NOT NULL DEFAULT 0,resume_policy TEXT NOT NULL DEFAULT 'continue',acceptance_json TEXT,metadata_json TEXT NOT NULL DEFAULT '{}',error TEXT,started_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,finished_at INTEGER,parent_run_id TEXT,recovery_plan_json TEXT,recovery_mode TEXT NOT NULL DEFAULT 'fresh',goal_contract_json TEXT,remediation_count INTEGER NOT NULL DEFAULT 0,heartbeat_at INTEGER,lease_expires_at INTEGER,quality_json TEXT);
             CREATE TABLE run_events(event_id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES agent_runs(run_id),conversation_id TEXT NOT NULL REFERENCES conversations(id),seq INTEGER NOT NULL,event_type TEXT NOT NULL,payload TEXT NOT NULL,created_at INTEGER NOT NULL,UNIQUE(run_id,seq));
             CREATE TABLE tool_runs(trace_id TEXT,status TEXT,recovery_policy TEXT);
             CREATE TABLE execution_steps(run_id TEXT,state TEXT,recovery_policy TEXT);",
        ).unwrap();
        c
    }

    #[test]
    fn transitions_are_ordered_and_replayable() {
        let c = conn();
        begin_run(&c, "r", "c", "goal").unwrap();
        transition(&c, "r", "c", "running", "executing_tool", None).unwrap();
        transition(&c, "r", "c", "completed", "done", None).unwrap();
        let run = get_run(&c, "r").unwrap().unwrap();
        assert_eq!(run.state, "completed");
        assert_eq!(run.last_event_seq, 3);
        let events = events_after(&c, "r", 1, 50).unwrap();
        assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn startup_marks_active_runs_interrupted() {
        let c = conn();
        begin_run(&c, "r", "c", "goal").unwrap();
        assert_eq!(recover_interrupted_runs(&c).unwrap(), 1);
        let run = get_run(&c, "r").unwrap().unwrap();
        assert_eq!(run.state, "interrupted");
        assert_eq!(run.recovery_count, 1);
        assert!(!matches!(
            run.state.as_str(),
            "queued" | "running" | "waiting_approval" | "waiting_user" | "verifying"
        ));
    }

    #[test]
    fn recovery_escalates_unknown_destructive_effects() {
        let c = conn();
        begin_run(&c, "r", "c", "deploy").unwrap();
        c.execute(
            "INSERT INTO tool_runs(trace_id,status,recovery_policy) VALUES ('r','interrupted','manual')",
            [],
        )
        .unwrap();
        recover_interrupted_runs(&c).unwrap();
        let run = get_run(&c, "r").unwrap().unwrap();
        assert_eq!(run.resume_policy, "manual");
    }

    #[test]
    fn terminal_state_cannot_be_overwritten_by_late_cleanup() {
        let c = conn();
        begin_run(&c, "r", "c", "goal").unwrap();
        transition(&c, "r", "c", "completed", "done", None).unwrap();
        transition(&c, "r", "c", "interrupted", "late_watchdog", Some("late")).unwrap();
        let run = get_run(&c, "r").unwrap().unwrap();
        assert_eq!(run.state, "completed");
        assert_eq!(run.phase, "done");
        assert_eq!(run.last_event_seq, 2);
    }

    #[test]
    fn managed_run_persists_contract_lease_and_remediation() {
        let c = conn();
        let contract = crate::agent::acceptance::GoalContract::compile("修复并测试");
        begin_managed_run(&c, "r", "c", "修复并测试", None, Some(&contract), 60_000).unwrap();
        let started = get_run(&c, "r").unwrap().unwrap();
        assert!(started.goal_contract_json.unwrap().contains("requested_change"));
        assert!(started.lease_expires_at.unwrap() > started.started_at);
        record_remediation(&c, "r", "c", &["测试通过".into()]).unwrap();
        assert_eq!(get_run(&c, "r").unwrap().unwrap().remediation_count, 1);
        transition(&c, "r", "c", "completed", "done", None).unwrap();
        assert!(get_run(&c, "r").unwrap().unwrap().lease_expires_at.is_none());
    }

    #[test]
    fn recovery_run_records_lineage_attempt_and_terminal_events() {
        let c = conn();
        begin_run(&c, "parent", "c", "ship product").unwrap();
        transition(
            &c,
            "parent",
            "c",
            "interrupted",
            "recovery_required",
            Some("process exited"),
        )
        .unwrap();
        let plan = crate::agent::recovery::RecoveryPlan {
            parent_run_id: "parent".into(),
            original_goal: "ship product".into(),
            policy: "verify_effects".into(),
            decisions: Vec::new(),
            completed_count: 0,
            pending_count: 0,
            verification_count: 0,
            confirmation_count: 0,
            created_at: 1,
        };
        begin_run_with_recovery(&c, "child", "c", "continue", Some(&plan)).unwrap();

        let child = get_run(&c, "child").unwrap().unwrap();
        assert_eq!(child.parent_run_id.as_deref(), Some("parent"));
        assert_eq!(child.recovery_mode, "resume");
        assert_eq!(child.resume_policy, "verify_effects");
        assert_eq!(child.attempt, 2);
        assert!(child.recovery_plan_json.is_some());
        let events = events_after(&c, "child", 0, 20).unwrap();
        assert_eq!(events[0].event_type, "run.started");
        assert_eq!(events[1].event_type, "recovery.planned");

        transition(&c, "child", "c", "completed", "done", None).unwrap();
        let events = events_after(&c, "child", 0, 20).unwrap();
        assert_eq!(events.last().unwrap().event_type, "recovery.completed");
        assert_eq!(get_run(&c, "child").unwrap().unwrap().last_event_seq, 4);
    }
}
