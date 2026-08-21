//! Run Coordinator 的持久化步骤层。
//!
//! 对话循环仍负责模型编排；本模块负责把计划项和真实工具调用收敛成可恢复状态，
//! 保证崩溃后能区分“尚未调用”“调用中断”“已完成”，而不是重新猜测整段任务。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::tools::contracts::ToolContract;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionStep {
    pub step_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub source: String,
    pub external_id: String,
    pub ordinal: i64,
    pub title: String,
    pub tool_name: Option<String>,
    pub input_hash: Option<String>,
    pub state: String,
    pub effect_kind: String,
    pub recovery_policy: String,
    pub verification_state: String,
    pub result_summary: Option<String>,
    pub started_at: Option<i64>,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn stable_hash(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

fn next_ordinal(conn: &Connection, run_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(ordinal),0)+1 FROM execution_steps WHERE run_id=?1",
        [run_id],
        |r| r.get(0),
    )
    .unwrap_or(1)
}

pub fn prepare_tool_step(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    call_id: &str,
    tool: &str,
    args: &str,
    contract: ToolContract,
) -> Result<(), String> {
    if run_id.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let ordinal = next_ordinal(conn, run_id);
    let step_id = format!("tool:{call_id}");
    let input_hash = stable_hash(&[tool, args]);
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO execution_steps
             (step_id,run_id,conversation_id,source,external_id,ordinal,title,tool_name,input_hash,
              state,effect_kind,recovery_policy,verification_state,updated_at)
             VALUES (?1,?2,?3,'tool',?4,?5,?6,?6,?7,'prepared',?8,?9,'not_started',?10)",
            params![
                step_id,
                run_id,
                conversation_id,
                call_id,
                ordinal,
                tool,
                input_hash,
                contract.effect.as_str(),
                contract.recovery.as_str(),
                now,
            ],
        )
        .map_err(|e| e.to_string())?;
    if inserted > 0 {
        let _ = crate::agent::runtime::append_event(
            conn,
            run_id,
            conversation_id,
            "step.prepared",
            serde_json::json!({
                "step_id": format!("tool:{call_id}"),
                "call_id": call_id,
                "tool": tool,
                "effect_kind": contract.effect.as_str(),
                "recovery_policy": contract.recovery.as_str(),
            }),
        );
    }
    Ok(())
}

pub fn start_tool_step(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    call_id: &str,
) -> Result<(), String> {
    if run_id.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let changed = conn
        .execute(
            "UPDATE execution_steps SET state='running',verification_state='in_progress',
             started_at=COALESCE(started_at,?1),updated_at=?1
             WHERE run_id=?2 AND source='tool' AND external_id=?3 AND state='prepared'",
            params![now, run_id, call_id],
        )
        .map_err(|e| e.to_string())?;
    if changed > 0 {
        let _ = crate::agent::runtime::append_event(
            conn,
            run_id,
            conversation_id,
            "step.started",
            serde_json::json!({ "step_id": format!("tool:{call_id}"), "call_id": call_id }),
        );
    }
    Ok(())
}

pub fn finish_tool_step(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    call_id: &str,
    status: &str,
    summary: &str,
) -> Result<(), String> {
    if run_id.is_empty() {
        return Ok(());
    }
    let effect_kind: Option<String> = conn
        .query_row(
            "SELECT effect_kind FROM execution_steps
             WHERE run_id=?1 AND source='tool' AND external_id=?2",
            params![run_id, call_id],
            |row| row.get(0),
        )
        .ok();
    let (state, verification) = match status {
        // 只读工具成功即可视为已核验；写入/破坏性工具成功只表示调用返回成功，
        // 真实产物仍由任务验收阶段确认，不能混淆“返回值”和“业务完成”。
        "ok" if effect_kind.as_deref() == Some("read") => ("completed", "verified"),
        "ok" => ("completed", "reported_success"),
        "blocked" => ("blocked", "not_started"),
        "cancelled" => ("cancelled", "unknown"),
        _ => ("failed", "failed"),
    };
    let now = now_ms();
    let summary: String = summary.chars().take(500).collect();
    let changed = conn
        .execute(
            "UPDATE execution_steps SET state=?1,verification_state=?2,result_summary=?3,
             updated_at=?4,finished_at=?4 WHERE run_id=?5 AND source='tool' AND external_id=?6
             AND state NOT IN ('completed','failed','blocked','cancelled','skipped')",
            params![state, verification, summary, now, run_id, call_id],
        )
        .map_err(|e| e.to_string())?;
    if changed > 0 {
        let _ = crate::agent::runtime::append_event(
            conn,
            run_id,
            conversation_id,
            "step.finished",
            serde_json::json!({
                "step_id": format!("tool:{call_id}"),
                "call_id": call_id,
                "state": state,
                "verification_state": verification,
            }),
        );
    }
    Ok(())
}

pub fn sync_todos(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    todos: &[crate::agent::todo::TodoItem],
) -> Result<(), String> {
    if run_id.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    for (index, todo) in todos.iter().enumerate() {
        let state = match todo.status.as_str() {
            "done" => "completed",
            "in_progress" => "running",
            _ => "pending",
        };
        let step_id = format!("plan:{}", stable_hash(&[run_id, &todo.id]));
        conn.execute(
            "INSERT INTO execution_steps
             (step_id,run_id,conversation_id,source,external_id,ordinal,title,state,effect_kind,
              recovery_policy,verification_state,started_at,updated_at,finished_at)
             VALUES (?1,?2,?3,'plan',?4,?5,?6,?7,'write','verify',?8,
                     CASE WHEN ?7='running' THEN ?9 ELSE NULL END,?9,
                     CASE WHEN ?7='completed' THEN ?9 ELSE NULL END)
             ON CONFLICT(run_id,source,external_id) DO UPDATE SET
               ordinal=excluded.ordinal,title=excluded.title,state=excluded.state,
               verification_state=excluded.verification_state,
               started_at=COALESCE(execution_steps.started_at,excluded.started_at),
               updated_at=excluded.updated_at,finished_at=excluded.finished_at",
            params![
                step_id,
                run_id,
                conversation_id,
                todo.id,
                index as i64 + 1,
                todo.content,
                state,
                if state == "completed" {
                    "declared_done"
                } else {
                    "not_started"
                },
                now,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    let _ = crate::agent::runtime::append_event(
        conn,
        run_id,
        conversation_id,
        "plan.synced",
        serde_json::json!({ "count": todos.len() }),
    );
    Ok(())
}

/// 将父 Run 的计划骨架继承到恢复 Run。完成项保持完成，其余一律回到 pending；工具步骤
/// 不复制，新的真实调用会在当前 Run 中重新落图，避免伪造已经执行过的调用。
pub fn inherit_plan_steps(
    conn: &Connection,
    parent_run_id: &str,
    run_id: &str,
    conversation_id: &str,
) -> Result<usize, String> {
    let todos: Vec<crate::agent::todo::TodoItem> = list_steps(conn, parent_run_id)?
        .into_iter()
        .filter(|step| step.source == "plan")
        .map(|step| crate::agent::todo::TodoItem {
            id: step.external_id,
            content: step.title,
            status: if step.state == "completed" {
                "done".into()
            } else {
                "pending".into()
            },
        })
        .collect();
    sync_todos(conn, run_id, conversation_id, &todos)?;
    Ok(todos.len())
}

/// Cancel inherited plan items that no longer belong to the amended goal.
/// A full replacement invalidates every unfinished parent plan item. For an
/// incremental amendment, only items that clearly name a removed criterion are
/// cancelled; ambiguous work remains pending for the Agent to inspect.
pub fn reconcile_goal_change(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    diff: &crate::agent::acceptance::GoalContractDiff,
) -> Result<usize, String> {
    if !diff.changed {
        return Ok(0);
    }
    let steps = list_steps(conn, run_id)?;
    let mut cancelled = 0usize;
    for step in steps.into_iter().filter(|step| {
        step.source == "plan" && matches!(step.state.as_str(), "pending" | "running" | "blocked")
    }) {
        let title = step.title.to_lowercase();
        let removed_match = diff.removed.iter().any(|criterion| {
            criterion_keywords(&criterion.kind).iter().any(|word| title.contains(word))
        });
        if diff.replacement || removed_match {
            cancelled += conn.execute(
                "UPDATE execution_steps SET state='cancelled',verification_state='not_applicable',
                 result_summary='目标契约变更后该计划项不再适用',updated_at=?1,finished_at=?1
                 WHERE step_id=?2 AND run_id=?3 AND state IN ('pending','running','blocked')",
                params![now_ms(), step.step_id, run_id],
            ).map_err(|e| e.to_string())?;
        }
    }
    let _ = crate::agent::runtime::append_event(
        conn,
        run_id,
        conversation_id,
        "goal.changed",
        serde_json::json!({"diff": diff, "cancelled_steps": cancelled}),
    );
    let _ = crate::agent::enterprise::audit(
        conn,
        Some(run_id),
        Some(conversation_id),
        "user",
        "goal.change",
        "goal_contract",
        "reconciled",
        &serde_json::json!({"diff": diff, "cancelled_steps": cancelled}),
    );
    Ok(cancelled)
}

fn criterion_keywords(kind: &crate::agent::acceptance::CriterionKind) -> &'static [&'static str] {
    use crate::agent::acceptance::CriterionKind;
    match kind {
        CriterionKind::Mutation => &["修改", "变更", "实现", "edit", "change", "implement"],
        CriterionKind::Verification => &["验证", "检查", "verify", "check"],
        CriterionKind::Build => &["构建", "编译", "build", "compile"],
        CriterionKind::Tests => &["测试", "用例", "test"],
        CriterionKind::Deploy => &["部署", "安装", "deploy", "install"],
        CriterionKind::GitCommit => &["提交", "commit"],
        CriterionKind::GitPush => &["推送", "push"],
    }
}

/// 进程重启恢复。prepared 明确表示尚未进入工具 Future，可安全标为未执行；running 才需要
/// 根据契约核验副作用。返回发生状态变化的步骤数。
pub fn recover_interrupted_steps(conn: &Connection) -> Result<usize, String> {
    let now = now_ms();
    let has_queue = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_task_queue')",
        [], |row| row.get::<_,bool>(0),
    ).unwrap_or(false);
    let recoverable = if has_queue {
        " AND run_id IN (SELECT run_id FROM agent_task_queue WHERE state='recovery_required')"
    } else { "" };
    let prepared = conn
        .execute(
            &format!("UPDATE execution_steps SET state='cancelled',verification_state='not_started',
             result_summary='应用退出时工具尚未开始执行',updated_at=?1,finished_at=?1
             WHERE source='tool' AND state='prepared'{recoverable}"),
            [now],
        )
        .map_err(|e| e.to_string())?;
    let running = conn
        .execute(
            &format!("UPDATE execution_steps SET state='interrupted',
             verification_state=CASE recovery_policy
               WHEN 'replay' THEN 'safe_to_replay'
               WHEN 'verify' THEN 'needs_verification'
               ELSE 'needs_manual_confirmation' END,
             result_summary='应用退出时工具仍在执行，实际副作用需按契约处理',
             updated_at=?1,finished_at=?1 WHERE source='tool' AND state='running'{recoverable}"),
            [now],
        )
        .map_err(|e| e.to_string())?;
    // todo 的 in_progress 只代表模型计划游标，并不证明对应工作已完成。重启后退回 pending，
    // 由新一轮 Agent 根据真实产物和验收条件决定从哪里继续。
    let plans = conn
        .execute(
            &format!("UPDATE execution_steps SET state='pending',verification_state='not_started',
             result_summary='应用退出时计划项仍在进行，恢复后需重新核验',updated_at=?1,
             started_at=NULL,finished_at=NULL WHERE source='plan' AND state='running'{recoverable}"),
            [now],
        )
        .map_err(|e| e.to_string())?;
    Ok(prepared + running + plans)
}

pub fn list_steps(conn: &Connection, run_id: &str) -> Result<Vec<ExecutionStep>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT step_id,run_id,conversation_id,source,external_id,ordinal,title,tool_name,
                    input_hash,state,effect_kind,recovery_policy,verification_state,result_summary,
                    started_at,updated_at,finished_at
             FROM execution_steps WHERE run_id=?1 ORDER BY ordinal,updated_at,step_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([run_id], |r| {
            Ok(ExecutionStep {
                step_id: r.get(0)?,
                run_id: r.get(1)?,
                conversation_id: r.get(2)?,
                source: r.get(3)?,
                external_id: r.get(4)?,
                ordinal: r.get(5)?,
                title: r.get(6)?,
                tool_name: r.get(7)?,
                input_hash: r.get(8)?,
                state: r.get(9)?,
                effect_kind: r.get(10)?,
                recovery_policy: r.get(11)?,
                verification_state: r.get(12)?,
                result_summary: r.get(13)?,
                started_at: r.get(14)?,
                updated_at: r.get(15)?,
                finished_at: r.get(16)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::contracts::{
        ApprovalPolicy, CancellationMode, EffectKind, IdempotencyKind, RecoveryPolicy,
    };

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE conversations(id TEXT PRIMARY KEY); INSERT INTO conversations VALUES('c');
             CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY,conversation_id TEXT,last_event_seq INTEGER DEFAULT 0,updated_at INTEGER);
             INSERT INTO agent_runs VALUES('r','c',0,0);
             CREATE TABLE run_events(event_id TEXT PRIMARY KEY,run_id TEXT,conversation_id TEXT,seq INTEGER,event_type TEXT,payload TEXT,created_at INTEGER,UNIQUE(run_id,seq));
             CREATE TABLE execution_steps(step_id TEXT PRIMARY KEY,run_id TEXT,conversation_id TEXT,source TEXT,external_id TEXT,ordinal INTEGER,title TEXT,tool_name TEXT,input_hash TEXT,state TEXT,effect_kind TEXT,recovery_policy TEXT,verification_state TEXT,result_summary TEXT,started_at INTEGER,updated_at INTEGER,finished_at INTEGER,UNIQUE(run_id,source,external_id));",
        ).unwrap();
        c
    }

    fn contract(recovery: RecoveryPolicy) -> ToolContract {
        ToolContract {
            effect: if recovery == RecoveryPolicy::Replay {
                EffectKind::Read
            } else {
                EffectKind::Write
            },
            recovery,
            retry_safe: false,
            idempotency: IdempotencyKind::Keyed,
            timeout_ms: 180_000,
            cancellation: CancellationMode::Cooperative,
            approval: ApprovalPolicy::ProjectTrust,
        }
    }

    #[test]
    fn prepared_and_running_have_different_recovery() {
        let c = conn();
        prepare_tool_step(
            &c,
            "r",
            "c",
            "a",
            "read_file",
            "{}",
            contract(RecoveryPolicy::Replay),
        )
        .unwrap();
        prepare_tool_step(
            &c,
            "r",
            "c",
            "b",
            "edit_file",
            "{}",
            contract(RecoveryPolicy::Verify),
        )
        .unwrap();
        start_tool_step(&c, "r", "c", "b").unwrap();
        assert_eq!(recover_interrupted_steps(&c).unwrap(), 2);
        let steps = list_steps(&c, "r").unwrap();
        assert_eq!(steps[0].verification_state, "not_started");
        assert_eq!(steps[1].verification_state, "needs_verification");
    }

    #[test]
    fn completed_step_is_terminal() {
        let c = conn();
        prepare_tool_step(
            &c,
            "r",
            "c",
            "a",
            "read_file",
            "{}",
            contract(RecoveryPolicy::Replay),
        )
        .unwrap();
        start_tool_step(&c, "r", "c", "a").unwrap();
        finish_tool_step(&c, "r", "c", "a", "ok", "done").unwrap();
        finish_tool_step(&c, "r", "c", "a", "error", "late").unwrap();
        assert_eq!(list_steps(&c, "r").unwrap()[0].state, "completed");
    }

    #[test]
    fn replacement_goal_cancels_only_unfinished_inherited_plan() {
        let c = conn();
        sync_todos(
            &c,
            "r",
            "c",
            &[
                crate::agent::todo::TodoItem { id: "done".into(), content: "实现功能".into(), status: "done".into() },
                crate::agent::todo::TodoItem { id: "deploy".into(), content: "部署到设备".into(), status: "pending".into() },
            ],
        ).unwrap();
        let previous = crate::agent::acceptance::GoalContract::compile("实现并部署");
        let (_, diff) = crate::agent::acceptance::GoalContract::reconcile(
            &previous,
            "目标改为只需要解释现有实现",
        );
        assert_eq!(reconcile_goal_change(&c, "r", "c", &diff).unwrap(), 1);
        let steps = list_steps(&c, "r").unwrap();
        assert_eq!(steps[0].state, "completed");
        assert_eq!(steps[1].state, "cancelled");
        assert_eq!(steps[1].verification_state, "not_applicable");
        let event: String = c.query_row(
            "SELECT event_type FROM run_events WHERE run_id='r' ORDER BY seq DESC LIMIT 1",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(event, "goal.changed");
    }

    #[test]
    fn running_plan_returns_to_pending_after_restart() {
        let c = conn();
        let todos = vec![crate::agent::todo::TodoItem {
            id: "one".into(),
            content: "implement".into(),
            status: "in_progress".into(),
        }];
        sync_todos(&c, "r", "c", &todos).unwrap();
        assert_eq!(recover_interrupted_steps(&c).unwrap(), 1);
        let step = list_steps(&c, "r").unwrap().remove(0);
        assert_eq!(step.state, "pending");
        assert_eq!(step.verification_state, "not_started");
    }

    #[test]
    fn successful_write_is_not_claimed_as_verified() {
        let c = conn();
        prepare_tool_step(
            &c,
            "r",
            "c",
            "a",
            "edit_file",
            "{}",
            contract(RecoveryPolicy::Verify),
        )
        .unwrap();
        start_tool_step(&c, "r", "c", "a").unwrap();
        finish_tool_step(&c, "r", "c", "a", "ok", "written").unwrap();
        assert_eq!(
            list_steps(&c, "r").unwrap()[0].verification_state,
            "reported_success"
        );
    }
}
