//! Recovery Orchestrator。
//!
//! 将上次 Run 的执行图转换为机器可读恢复决策。模型只负责按计划完成任务，不能自行
//! 猜测哪些步骤可以重放；Run 血缘、完成项跳过和副作用处理策略由运行内核决定。

use serde::{Deserialize, Serialize};

use crate::agent::coordinator::ExecutionStep;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    SkipCompleted,
    ResumePending,
    ReplayRead,
    VerifyEffect,
    AwaitConfirmation,
    InspectFailure,
}

impl RecoveryAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SkipCompleted => "skip_completed",
            Self::ResumePending => "resume_pending",
            Self::ReplayRead => "replay_read",
            Self::VerifyEffect => "verify_effect",
            Self::AwaitConfirmation => "await_confirmation",
            Self::InspectFailure => "inspect_failure",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryDecision {
    pub step_id: String,
    pub source: String,
    pub title: String,
    pub previous_state: String,
    pub verification_state: String,
    pub recovery_policy: String,
    pub action: RecoveryAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryPlan {
    pub parent_run_id: String,
    pub original_goal: String,
    pub policy: String,
    pub decisions: Vec<RecoveryDecision>,
    pub completed_count: usize,
    pub pending_count: usize,
    pub verification_count: usize,
    pub confirmation_count: usize,
    pub created_at: i64,
}

fn action_for(step: &ExecutionStep) -> RecoveryAction {
    if matches!(step.state.as_str(), "completed" | "skipped") {
        return RecoveryAction::SkipCompleted;
    }
    if matches!(step.state.as_str(), "interrupted" | "failed" | "cancelled") {
        return match step.recovery_policy.as_str() {
            "replay" => RecoveryAction::ReplayRead,
            "manual" => RecoveryAction::AwaitConfirmation,
            _ => RecoveryAction::VerifyEffect,
        };
    }
    if step.state == "blocked" {
        return RecoveryAction::InspectFailure;
    }
    RecoveryAction::ResumePending
}

pub fn build_plan(
    conn: &rusqlite::Connection,
    conversation_id: &str,
    parent_run_id: &str,
) -> Result<RecoveryPlan, String> {
    let parent = crate::agent::runtime::get_run(conn, parent_run_id)?
        .ok_or_else(|| format!("要恢复的运行不存在：{parent_run_id}"))?;
    if parent.conversation_id != conversation_id {
        return Err("恢复运行不属于当前会话".into());
    }
    if parent.state == "completed" {
        return Err("该任务已经完成，无需恢复".into());
    }
    if !matches!(
        parent.state.as_str(),
        "interrupted" | "cancelled" | "failed"
    ) {
        return Err(format!("运行当前状态为 {}，不能创建恢复分支", parent.state));
    }
    let decisions: Vec<RecoveryDecision> =
        crate::agent::coordinator::list_steps(conn, parent_run_id)?
            .into_iter()
            .map(|step| RecoveryDecision {
                action: action_for(&step),
                step_id: step.step_id,
                source: step.source,
                title: step.title,
                previous_state: step.state,
                verification_state: step.verification_state,
                recovery_policy: step.recovery_policy,
            })
            .collect();
    let completed_count = decisions
        .iter()
        .filter(|item| item.action == RecoveryAction::SkipCompleted)
        .count();
    let verification_count = decisions
        .iter()
        .filter(|item| item.action == RecoveryAction::VerifyEffect)
        .count();
    let confirmation_count = decisions
        .iter()
        .filter(|item| item.action == RecoveryAction::AwaitConfirmation)
        .count();
    let pending_count = decisions.len().saturating_sub(completed_count);
    let policy = if confirmation_count > 0 {
        "manual"
    } else if verification_count > 0 {
        "verify_effects"
    } else if decisions.is_empty()
        && matches!(parent.resume_policy.as_str(), "manual" | "verify_effects")
    {
        parent.resume_policy.as_str()
    } else {
        "continue"
    };
    Ok(RecoveryPlan {
        parent_run_id: parent.run_id,
        original_goal: parent.goal,
        policy: policy.into(),
        decisions,
        completed_count,
        pending_count,
        verification_count,
        confirmation_count,
        created_at: chrono::Utc::now().timestamp_millis(),
    })
}

pub fn directive(plan: &RecoveryPlan) -> String {
    let mut out = format!(
        "## 运行内核恢复计划（强制执行）\n父运行：{}\n原始目标：{}\n恢复策略：{}\n\
         规则：已完成步骤不得重复执行；verify_effect 必须先用只读工具核验真实状态；\
         await_confirmation 对应操作不得盲目重放，若确需再次执行必须走正常危险操作审批。\n",
        plan.parent_run_id, plan.original_goal, plan.policy
    );
    for decision in plan.decisions.iter().take(40) {
        out.push_str(&format!(
            "- [{}] {}（之前={}，核验={}，策略={}）\n",
            decision.action.as_str(),
            decision.title,
            decision.previous_state,
            decision.verification_state,
            decision.recovery_policy,
        ));
    }
    if plan.decisions.len() > 40 {
        out.push_str(&format!(
            "- 其余 {} 个步骤已省略，仍以持久化执行图为准\n",
            plan.decisions.len() - 40
        ));
    }
    out
}

fn current_plan(conn: &rusqlite::Connection, run_id: &str) -> Option<RecoveryPlan> {
    crate::agent::runtime::get_run(conn, run_id)
        .ok()
        .flatten()
        .and_then(|run| run.recovery_plan_json)
        .and_then(|json| serde_json::from_str(&json).ok())
}

fn requires_confirmation(conn: &rusqlite::Connection, run_id: &str, tool: &str) -> bool {
    current_plan(conn, run_id).is_some_and(|plan| {
        let exact_manual_step = plan.decisions.iter().any(|item| {
            item.source == "tool"
                && item.title == tool
                && item.action == RecoveryAction::AwaitConfirmation
        });
        exact_manual_step || (plan.decisions.is_empty() && plan.policy == "manual")
    })
}

/// 不可重放的同名工具即使处于 allow_all 或历史白名单中，也必须重新走本次审批。
pub fn requires_confirmation_global(run_id: &str, tool: &str) -> bool {
    let Some(db) = crate::db::global() else {
        return false;
    };
    let Ok(conn) = db.lock() else { return false };
    requires_confirmation(&conn, run_id, tool)
}

fn verification_block(conn: &rusqlite::Connection, run_id: &str, tool: &str) -> Option<String> {
    if crate::agent::tools::contracts::contract(tool).effect
        == crate::agent::tools::contracts::EffectKind::Read
    {
        return None;
    }
    let plan = current_plan(conn, run_id)?;
    let needs_verification = plan
        .decisions
        .iter()
        .any(|item| item.action == RecoveryAction::VerifyEffect)
        || (plan.decisions.is_empty() && plan.policy == "verify_effects");
    if !needs_verification {
        return None;
    }
    let has_read_evidence: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM execution_steps
             WHERE run_id=?1 AND source='tool' AND effect_kind='read' AND state='completed')",
            [run_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    (!has_read_evidence).then(|| {
        format!(
            "恢复安全门已阻止写入工具 {tool}：父任务存在尚未核验的写入副作用。请先调用只读工具检查文件、Git、设备或外部系统的真实状态，再根据结果继续。"
        )
    })
}

/// 恢复计划包含未知写入副作用时，在本 Run 至少取得一条成功的只读工具证据之前，
/// 阻止新的写入调用。返回文案意味着调用方应把它作为普通工具结果反馈模型并继续编排。
pub fn verification_block_global(run_id: &str, tool: &str) -> Option<String> {
    if crate::agent::tools::contracts::contract(tool).effect
        == crate::agent::tools::contracts::EffectKind::Read
    {
        return None;
    }
    let db = crate::db::global()?;
    let conn = db.lock().ok()?;
    verification_block(&conn, run_id, tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE conversations(id TEXT PRIMARY KEY);
             INSERT INTO conversations(id) VALUES ('c'),('other');
             CREATE TABLE agent_runs(
               run_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, goal TEXT NOT NULL,
               state TEXT NOT NULL, phase TEXT NOT NULL, attempt INTEGER NOT NULL DEFAULT 1,
               last_event_seq INTEGER NOT NULL DEFAULT 0, recovery_count INTEGER NOT NULL DEFAULT 0,
               resume_policy TEXT NOT NULL DEFAULT 'continue', acceptance_json TEXT, error TEXT,
               started_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, finished_at INTEGER,
               parent_run_id TEXT, recovery_plan_json TEXT, recovery_mode TEXT NOT NULL DEFAULT 'fresh'
             );
             CREATE TABLE execution_steps(
               step_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, conversation_id TEXT NOT NULL,
               source TEXT NOT NULL, external_id TEXT NOT NULL, ordinal INTEGER NOT NULL,
               title TEXT NOT NULL, tool_name TEXT, input_hash TEXT, state TEXT NOT NULL,
               effect_kind TEXT NOT NULL, recovery_policy TEXT NOT NULL,
               verification_state TEXT NOT NULL, result_summary TEXT, started_at INTEGER,
               updated_at INTEGER NOT NULL, finished_at INTEGER
             );",
        )
        .unwrap();
        conn
    }

    fn insert_run(conn: &rusqlite::Connection, id: &str, conversation: &str, state: &str) {
        conn.execute(
            "INSERT INTO agent_runs(run_id,conversation_id,goal,state,phase,started_at,updated_at)
             VALUES (?1,?2,'ship product',?3,'recovery_required',1,1)",
            rusqlite::params![id, conversation, state],
        )
        .unwrap();
    }

    fn step(state: &str, recovery: &str) -> ExecutionStep {
        ExecutionStep {
            step_id: "s".into(),
            run_id: "r".into(),
            conversation_id: "c".into(),
            source: "tool".into(),
            external_id: "x".into(),
            ordinal: 1,
            title: "tool".into(),
            tool_name: Some("tool".into()),
            input_hash: None,
            state: state.into(),
            effect_kind: "write".into(),
            recovery_policy: recovery.into(),
            verification_state: "unknown".into(),
            result_summary: None,
            started_at: None,
            updated_at: 0,
            finished_at: None,
        }
    }

    #[test]
    fn recovery_matrix_is_conservative() {
        assert_eq!(
            action_for(&step("completed", "manual")),
            RecoveryAction::SkipCompleted
        );
        assert_eq!(
            action_for(&step("interrupted", "replay")),
            RecoveryAction::ReplayRead
        );
        assert_eq!(
            action_for(&step("interrupted", "verify")),
            RecoveryAction::VerifyEffect
        );
        assert_eq!(
            action_for(&step("interrupted", "manual")),
            RecoveryAction::AwaitConfirmation
        );
        assert_eq!(
            action_for(&step("failed", "replay")),
            RecoveryAction::ReplayRead
        );
        assert_eq!(
            action_for(&step("failed", "manual")),
            RecoveryAction::AwaitConfirmation
        );
        assert_eq!(
            action_for(&step("cancelled", "verify")),
            RecoveryAction::VerifyEffect
        );
        assert_eq!(
            action_for(&step("blocked", "manual")),
            RecoveryAction::InspectFailure
        );
    }

    #[test]
    fn plan_rejects_cross_conversation_and_completed_runs() {
        let conn = conn();
        insert_run(&conn, "foreign", "other", "interrupted");
        insert_run(&conn, "done", "c", "completed");
        assert!(build_plan(&conn, "c", "foreign")
            .unwrap_err()
            .contains("不属于当前会话"));
        assert!(build_plan(&conn, "c", "done")
            .unwrap_err()
            .contains("已经完成"));
    }

    #[test]
    fn recovery_gates_side_effects_until_read_evidence_exists() {
        let conn = conn();
        insert_run(&conn, "parent", "c", "interrupted");
        conn.execute(
            "INSERT INTO execution_steps
             (step_id,run_id,conversation_id,source,external_id,ordinal,title,tool_name,state,
              effect_kind,recovery_policy,verification_state,updated_at)
             VALUES
             ('verify','parent','c','tool','v',1,'edit_file','edit_file','interrupted','write','verify','unknown',1),
             ('manual','parent','c','tool','m',2,'git_push','git_push','interrupted','external','manual','unknown',1)",
            [],
        )
        .unwrap();
        let plan = build_plan(&conn, "c", "parent").unwrap();
        assert_eq!(plan.policy, "manual");
        assert_eq!(plan.verification_count, 1);
        assert_eq!(plan.confirmation_count, 1);

        insert_run(&conn, "child", "c", "running");
        conn.execute(
            "UPDATE agent_runs SET recovery_plan_json=?1,recovery_mode='resume',parent_run_id='parent'
             WHERE run_id='child'",
            [serde_json::to_string(&plan).unwrap()],
        )
        .unwrap();
        assert!(requires_confirmation(&conn, "child", "git_push"));
        assert!(!requires_confirmation(&conn, "child", "edit_file"));
        assert!(verification_block(&conn, "child", "edit_file").is_some());
        assert!(verification_block(&conn, "child", "read_file").is_none());

        conn.execute(
            "INSERT INTO execution_steps
             (step_id,run_id,conversation_id,source,external_id,ordinal,title,tool_name,state,
              effect_kind,recovery_policy,verification_state,updated_at)
             VALUES ('read','child','c','tool','r',1,'read_file','read_file','completed','read','replay','verified',2)",
            [],
        )
        .unwrap();
        assert!(verification_block(&conn, "child", "edit_file").is_none());
    }

    #[test]
    fn legacy_run_keeps_persisted_conservative_policy() {
        let conn = conn();
        insert_run(&conn, "parent", "c", "interrupted");
        conn.execute(
            "UPDATE agent_runs SET resume_policy='verify_effects' WHERE run_id='parent'",
            [],
        )
        .unwrap();
        let plan = build_plan(&conn, "c", "parent").unwrap();
        assert_eq!(plan.policy, "verify_effects");
        assert!(plan.decisions.is_empty());
    }
}
