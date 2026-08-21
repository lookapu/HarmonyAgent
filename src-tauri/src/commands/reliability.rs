use serde::Serialize;
use tauri::State;

use crate::db::DbState;

#[derive(Clone, Debug, Serialize)]
pub struct NamedCount {
    pub name: String,
    pub count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct QualityRunRow {
    pub run_id: String,
    pub conversation_id: String,
    pub goal: String,
    pub state: String,
    pub score: Option<u8>,
    pub acceptance_passed: Option<bool>,
    pub remediation_count: i64,
    pub recovered: bool,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReliabilityDashboard {
    pub total_runs: i64,
    pub acceptance_rate: f64,
    pub average_quality_score: f64,
    pub remediation_success_rate: f64,
    pub recovery_success_rate: f64,
    pub false_completion_count: i64,
    pub structured_evidence_coverage: f64,
    pub duplicate_side_effect_count: i64,
    pub scheduler_states: Vec<NamedCount>,
    pub dag_total_nodes: i64,
    pub dag_completed_nodes: i64,
    pub dag_failed_nodes: i64,
    pub latest_eval: Option<crate::agent::evals::EvalRun>,
    pub recent_runs: Vec<QualityRunRow>,
    pub open_alert_count: i64,
    pub critical_alert_count: i64,
    pub quota: crate::agent::enterprise::QuotaUsage,
    pub worker_runtime: crate::agent::scheduler::WorkerRuntimeStats,
    pub tool_runtime: crate::agent::tool_runtime::ToolRuntimeStats,
    pub tool_governance: Vec<crate::agent::tool_governance::ToolGovernanceItem>,
    pub tool_quality: crate::agent::tool_metrics::ToolQualitySummary,
    pub tool_metric_slices: Vec<crate::agent::tool_metrics::ToolMetricSlice>,
    pub tool_protocol_versions: Vec<crate::agent::tool_metrics::ToolProtocolVersion>,
}

#[tauri::command]
pub fn get_reliability_dashboard(
    db: State<DbState>,
    days: Option<i64>,
) -> Result<ReliabilityDashboard, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let _ = crate::agent::scheduler::recover_expired(&conn);
    let since =
        chrono::Utc::now().timestamp_millis() - days.unwrap_or(30).clamp(1, 365) * 86_400_000;
    let _ = crate::agent::enterprise::evaluate_window_slo(&conn, since);
    let mut stmt = conn.prepare(
        "SELECT run_id,conversation_id,goal,state,quality_json,acceptance_json,remediation_count,recovery_mode,updated_at
         FROM agent_runs WHERE parent_run_id IS NULL AND started_at>=?1 ORDER BY started_at DESC",
    ).map_err(|e| e.to_string())?;
    let raw = stmt
        .query_map([since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let total_runs = raw.len() as i64;
    let mut accepted = 0i64;
    let mut quality_total = 0i64;
    let mut quality_count = 0i64;
    let mut remediated = 0i64;
    let mut remediated_success = 0i64;
    let mut recovered = 0i64;
    let mut recovered_success = 0i64;
    let mut false_completion_count = 0i64;
    let mut recent_runs = Vec::new();
    for (
        run_id,
        conversation_id,
        goal,
        state,
        quality_json,
        acceptance_json,
        remediation_count,
        recovery_mode,
        updated_at,
    ) in raw
    {
        let quality = quality_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok());
        let acceptance = acceptance_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok());
        let passed = acceptance
            .as_ref()
            .and_then(|value| value["passed"].as_bool())
            .or_else(|| {
                quality
                    .as_ref()
                    .and_then(|value| value["acceptance_passed"].as_bool())
            });
        let score = quality
            .as_ref()
            .and_then(|value| value["score"].as_u64())
            .map(|value| value as u8);
        if passed == Some(true) {
            accepted += 1;
        }
        if let Some(score) = score {
            quality_total += i64::from(score);
            quality_count += 1;
        }
        if remediation_count > 0 {
            remediated += 1;
            if passed == Some(true) {
                remediated_success += 1;
            }
        }
        let was_recovered = recovery_mode == "resume";
        if was_recovered {
            recovered += 1;
            if state == "completed" && passed == Some(true) {
                recovered_success += 1;
            }
        }
        if state == "completed" && passed == Some(false) {
            false_completion_count += 1;
        }
        if recent_runs.len() < 30 {
            recent_runs.push(QualityRunRow {
                run_id,
                conversation_id,
                goal,
                state,
                score,
                acceptance_passed: passed,
                remediation_count,
                recovered: was_recovered,
                updated_at,
            });
        }
    }
    drop(stmt);
    let terminal_tools: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_runs WHERE created_at>=?1 AND status IN ('ok','error','blocked','cancelled','interrupted')",
        [since / 1000], |row| row.get(0),
    ).unwrap_or(0);
    let structured_tools: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_runs WHERE created_at>=?1 AND structured_result_json IS NOT NULL
         AND (protocol_version<2 OR outcome_committed_at IS NOT NULL)
         AND status IN ('ok','error','blocked','cancelled','interrupted')", [since / 1000], |row| row.get(0),
    ).unwrap_or(0);
    let duplicate_side_effect_count: i64 = conn.query_row(
        "SELECT COALESCE(SUM(n-1),0) FROM (SELECT COUNT(*) n FROM tool_runs WHERE created_at>=?1
         AND effect_kind!='read' AND status='ok' AND idempotency_key IS NOT NULL
         GROUP BY trace_id,idempotency_key HAVING COUNT(*)>1)",
        [since / 1000], |row| row.get(0),
    ).unwrap_or(0);
    let scheduler_states = named_counts(
        &conn,
        "SELECT state,COUNT(*) FROM agent_task_queue GROUP BY state",
    )?;
    let (dag_total_nodes, dag_completed_nodes, dag_failed_nodes) = conn.query_row(
        "SELECT COUNT(*),SUM(CASE WHEN state='completed' THEN 1 ELSE 0 END),SUM(CASE WHEN state='failed' THEN 1 ELSE 0 END)
         FROM agent_dag_nodes WHERE created_at>=?1", [since], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).unwrap_or((0,0,0));
    let (open_alert_count,critical_alert_count) = conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(CASE WHEN severity='critical' THEN 1 ELSE 0 END),0) FROM agent_alerts WHERE state='open'",
        [], |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap_or((0,0));
    Ok(ReliabilityDashboard {
        total_runs,
        acceptance_rate: ratio(accepted, total_runs),
        average_quality_score: if quality_count == 0 {
            0.0
        } else {
            quality_total as f64 / quality_count as f64
        },
        remediation_success_rate: ratio(remediated_success, remediated),
        recovery_success_rate: ratio(recovered_success, recovered),
        false_completion_count,
        structured_evidence_coverage: ratio(structured_tools, terminal_tools),
        duplicate_side_effect_count,
        scheduler_states,
        dag_total_nodes,
        dag_completed_nodes,
        dag_failed_nodes,
        latest_eval: latest_eval(&conn)?,
        recent_runs,
        open_alert_count,
        critical_alert_count,
        quota: crate::agent::enterprise::quota(&conn)?,
        worker_runtime: crate::agent::scheduler::runtime_stats(&conn)?,
        tool_runtime: crate::agent::tool_runtime::runtime_stats(&conn)?,
        tool_governance: crate::agent::tool_governance::report(&conn, since / 1000)?,
        tool_quality: crate::agent::tool_metrics::summary(&conn, since / 1000)?,
        tool_metric_slices: crate::agent::tool_metrics::breakdown(&conn, since / 1000)?,
        tool_protocol_versions: crate::agent::tool_metrics::protocol_versions(&conn)?,
    })
}

#[tauri::command]
pub fn list_agent_alerts(
    db: State<DbState>,
    limit: Option<usize>,
) -> Result<Vec<crate::agent::enterprise::AgentAlert>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::enterprise::list_alerts(&conn, limit.unwrap_or(100))
}

#[tauri::command]
pub fn list_agent_audit_events(
    db: State<DbState>,
    run_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<crate::agent::enterprise::AuditEvent>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::enterprise::list_audit(&conn, run_id.as_deref(), limit.unwrap_or(200))
}

#[tauri::command]
pub fn list_agent_workers(
    db: State<DbState>,
    limit: Option<usize>,
) -> Result<Vec<crate::agent::scheduler::WorkerInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::list_workers(&conn, limit.unwrap_or(100))
}

#[tauri::command]
pub fn list_tool_execution_workers(
    db: State<DbState>,
    limit: Option<usize>,
) -> Result<Vec<crate::agent::tool_runtime::ToolWorkerInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::tool_runtime::list_workers(&conn, limit.unwrap_or(100))
}

#[tauri::command]
pub fn get_agent_slo_policy(
    db: State<DbState>,
) -> Result<Option<crate::agent::enterprise::SloPolicy>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::enterprise::get_policy(&conn)
}

#[tauri::command]
pub fn update_agent_slo_policy(
    db: State<DbState>,
    policy: crate::agent::enterprise::SloPolicy,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::enterprise::update_policy(&conn, &policy)?;
    crate::agent::enterprise::audit(
        &conn,
        None,
        None,
        "user",
        "slo.update",
        "agent_slo_policy",
        "success",
        &serde_json::json!({"policy_id":policy.policy_id}),
    )
}

#[tauri::command]
pub fn run_reliability_evaluation(
    db: State<DbState>,
    threshold: Option<f64>,
) -> Result<crate::agent::evals::EvalRun, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::evals::run_suite(
        Some(&conn),
        threshold.unwrap_or(crate::agent::evals::DEFAULT_RELIABILITY_THRESHOLD),
    )
}

#[tauri::command]
pub fn list_scheduled_agent_tasks(
    db: State<DbState>,
    limit: Option<usize>,
) -> Result<Vec<crate::agent::scheduler::ScheduledTask>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::list(&conn, limit.unwrap_or(100))
}

#[tauri::command]
pub fn list_agent_dag_nodes(
    db: State<DbState>,
    root_run_id: String,
) -> Result<Vec<crate::agent::dag::DagNode>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::dag::list_nodes(&conn, &root_run_id)
}

#[tauri::command]
pub fn get_scheduled_agent_task(
    db: State<DbState>,
    run_id: String,
) -> Result<Option<crate::agent::scheduler::ScheduledTask>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::get(&conn, &run_id)
}

#[tauri::command]
pub fn claim_next_scheduled_agent_task(
    db: State<DbState>,
    lease_ms: Option<i64>,
) -> Result<Option<crate::agent::scheduler::ScheduledTask>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::claim_next(&conn, lease_ms.unwrap_or(180_000))
}

#[tauri::command]
pub fn retry_scheduled_agent_task(
    db: State<DbState>,
    run_id: String,
    error: String,
    delay_ms: Option<i64>,
) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::release_for_retry(&conn, &run_id, &error, delay_ms.unwrap_or(1_000))
}

#[tauri::command]
pub fn resume_scheduled_agent_task(
    db: State<DbState>,
    run_id: String,
    resume_token: String,
) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::request_resume(&conn, &run_id, &resume_token)
}

#[tauri::command]
pub fn pause_scheduled_agent_task(db: State<DbState>, run_id: String) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::request_pause(&conn, &run_id)
}

#[tauri::command]
pub fn cancel_scheduled_agent_task(db: State<DbState>, run_id: String) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::scheduler::request_cancel(&conn, &run_id)
}

#[tauri::command]
pub fn add_agent_dag_dependency(
    db: State<DbState>,
    root_run_id: String,
    from_node_id: String,
    to_node_id: String,
    required: Option<bool>,
    condition: Option<serde_json::Value>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::dag::add_dependency(
        &conn,
        &root_run_id,
        &from_node_id,
        &to_node_id,
        required.unwrap_or(true),
        &condition.unwrap_or_default(),
    )
}

#[tauri::command]
pub fn list_runnable_agent_dag_nodes(
    db: State<DbState>,
    root_run_id: String,
    limit: Option<usize>,
) -> Result<Vec<crate::agent::dag::DagNode>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::dag::runnable_nodes(&conn, &root_run_id, limit.unwrap_or(50))
}

#[tauri::command]
pub fn retry_agent_dag_node(
    db: State<DbState>,
    node_id: String,
    error: String,
    delay_ms: Option<i64>,
) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::dag::fail_or_retry_node(&conn, &node_id, &error, delay_ms.unwrap_or(1_000))
}

#[tauri::command]
pub fn cancel_agent_dag_descendants(
    db: State<DbState>,
    root_run_id: String,
    node_id: String,
    reason: String,
) -> Result<usize, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::agent::dag::cancel_descendants(&conn, &root_run_id, &node_id, &reason)
}

fn ratio(numerator: i64, denominator: i64) -> f64 {
    if denominator <= 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn named_counts(conn: &rusqlite::Connection, sql: &str) -> Result<Vec<NamedCount>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(NamedCount {
                name: row.get(0)?,
                count: row.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn latest_eval(
    conn: &rusqlite::Connection,
) -> Result<Option<crate::agent::evals::EvalRun>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT eval_run_id,suite,platform,passed,total_cases,passed_cases,score,threshold,results_json,created_at
         FROM agent_eval_runs ORDER BY created_at DESC LIMIT 1", [], |row| {
            let raw: String = row.get(8)?;
            Ok(crate::agent::evals::EvalRun {
                eval_run_id: row.get(0)?, suite: row.get(1)?, platform: row.get(2)?, passed: row.get(3)?,
                total_cases: row.get::<_, i64>(4)? as usize, passed_cases: row.get::<_, i64>(5)? as usize,
                score: row.get(6)?, threshold: row.get(7)?, results: serde_json::from_str(&raw).unwrap_or_default(),
                created_at: row.get(9)?,
            })
        },
    ).optional().map_err(|e| e.to_string())
}
