//! 工具质量指标真源：所有比率、面板切片和最终目标贡献都由 durable `tool_runs` 计算。

use rusqlite::{params, Connection};
use serde::Serialize;

const TERMINAL: &str = "'ok','error','blocked','cancelled','interrupted'";

#[derive(Clone, Debug, Default, Serialize)]
pub struct ToolQualitySummary {
    pub total_calls: i64,
    pub successful_calls: i64,
    pub success_rate: f64,
    pub argument_error_rate: f64,
    pub timeout_rate: f64,
    pub retry_rate: f64,
    pub cancellation_count: i64,
    pub average_cancellation_latency_ms: Option<f64>,
    pub average_duration_ms: f64,
    pub contributing_success_rate: f64,
    pub side_effect_repeat_rate: f64,
    pub wrong_tool_selection_rate: f64,
    pub ineffective_call_rate: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolMetricSlice {
    pub dimension: String,
    pub value: String,
    pub calls: i64,
    pub successes: i64,
    pub success_rate: f64,
    pub contribution_rate: f64,
    pub average_duration_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolProtocolVersion {
    pub schema_version: i64,
    pub status: String,
    pub min_reader_version: i64,
    pub producer_version: String,
    pub compatibility: String,
    pub migration_notes: String,
}

fn ratio(part: i64, total: i64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 / total as f64
    }
}

pub fn summary(conn: &Connection, since_seconds: i64) -> Result<ToolQualitySummary, String> {
    let sql = format!(
        "SELECT COUNT(*),
          COALESCE(SUM(status='ok'),0),
          COALESCE(SUM(error_code='TOOL_ARGUMENT_INVALID'),0),
          COALESCE(SUM(error_code='TOOL_TIMEOUT'),0),
          COALESCE(SUM(retry_count>0),0),
          COALESCE(SUM(status='cancelled'),0),
          AVG(CASE WHEN cancellation_latency_ms IS NOT NULL THEN cancellation_latency_ms END),
          COALESCE(AVG(COALESCE(duration_ms,0)),0),
          COALESCE(SUM(status='ok' AND contribution_state='contributing'),0),
          COALESCE(SUM(selection_state='out_of_capability_pack'),0),
          COALESCE(SUM(status='ok' AND contribution_state='no_direct_acceptance_evidence'),0)
         FROM tool_runs WHERE created_at>=?1 AND status IN ({TERMINAL})"
    );
    let row = conn
        .query_row(&sql, [since_seconds], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let successful_side_effects: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_runs WHERE created_at>=?1 AND status='ok' AND effect_kind!='read'",
        [since_seconds], |row| row.get(0),
    ).unwrap_or(0);
    let repeated_side_effects: i64 = conn.query_row(
        "SELECT COALESCE(SUM(n-1),0) FROM (SELECT COUNT(*) n FROM tool_runs
         WHERE created_at>=?1 AND status='ok' AND effect_kind!='read' AND idempotency_key IS NOT NULL
         GROUP BY trace_id,idempotency_key HAVING COUNT(*)>1)",
        [since_seconds], |row| row.get(0),
    ).unwrap_or(0);
    Ok(ToolQualitySummary {
        total_calls: row.0,
        successful_calls: row.1,
        success_rate: ratio(row.1, row.0),
        argument_error_rate: ratio(row.2, row.0),
        timeout_rate: ratio(row.3, row.0),
        retry_rate: ratio(row.4, row.0),
        cancellation_count: row.5,
        average_cancellation_latency_ms: row.6,
        average_duration_ms: row.7,
        contributing_success_rate: ratio(row.8, row.1),
        side_effect_repeat_rate: ratio(repeated_side_effects, successful_side_effects),
        wrong_tool_selection_rate: ratio(row.9, row.0),
        ineffective_call_rate: ratio(row.10, row.1),
    })
}

pub fn breakdown(conn: &Connection, since_seconds: i64) -> Result<Vec<ToolMetricSlice>, String> {
    let dimensions = [
        ("tool", "tool_name"),
        ("capability_pack", "COALESCE(NULLIF(capability_pack,''),'unknown')"),
        ("model", "COALESCE(NULLIF(model,''),'unknown')"),
        ("project", "COALESCE(NULLIF(project_id,''),'unknown')"),
        ("version", "printf('protocol-v%d / %s',protocol_version,COALESCE(NULLIF(producer_version,''),'legacy'))"),
    ];
    let mut output = Vec::new();
    for (dimension, expression) in dimensions {
        let sql = format!(
            "SELECT {expression},COUNT(*),COALESCE(SUM(status='ok'),0),
             COALESCE(SUM(status='ok' AND contribution_state='contributing'),0),
             COALESCE(AVG(COALESCE(duration_ms,0)),0)
             FROM tool_runs WHERE created_at>=?1 AND status IN ({TERMINAL})
             GROUP BY {expression} ORDER BY COUNT(*) DESC,{expression} LIMIT 100"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([since_seconds], |row| {
                let calls = row.get::<_, i64>(1)?;
                let successes = row.get::<_, i64>(2)?;
                let contributions = row.get::<_, i64>(3)?;
                Ok(ToolMetricSlice {
                    dimension: dimension.into(),
                    value: row.get(0)?,
                    calls,
                    successes,
                    success_rate: ratio(successes, calls),
                    contribution_rate: ratio(contributions, successes),
                    average_duration_ms: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        output.extend(rows.filter_map(Result::ok));
    }
    Ok(output)
}

pub fn protocol_versions(conn: &Connection) -> Result<Vec<ToolProtocolVersion>, String> {
    let mut stmt = conn.prepare(
        "SELECT schema_version,status,min_reader_version,producer_version,compatibility,migration_notes
         FROM tool_protocol_versions ORDER BY schema_version"
    ).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ToolProtocolVersion {
                schema_version: row.get(0)?,
                status: row.get(1)?,
                min_reader_version: row.get(2)?,
                producer_version: row.get(3)?,
                compatibility: row.get(4)?,
                migration_notes: row.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn record_attempt_metrics(
    conn: &Connection,
    call_id: &str,
    retry_count: i64,
    cancel_requested_at: Option<i64>,
) -> Result<(), String> {
    let observed = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "UPDATE tool_runs SET retry_count=?1,
         cancel_requested_at=CASE WHEN status='cancelled' THEN ?2 ELSE cancel_requested_at END,
         cancel_observed_at=CASE WHEN status='cancelled' AND ?2 IS NOT NULL THEN ?3 ELSE cancel_observed_at END,
         cancellation_latency_ms=CASE WHEN status='cancelled' AND ?2 IS NOT NULL THEN MAX(?3-?2,0) ELSE cancellation_latency_ms END
         WHERE id=?4",
        params![retry_count.max(0), cancel_requested_at, observed, call_id],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn annotate_run_outcomes(
    conn: &Connection,
    root_run_id: &str,
    goal: &str,
    model: &str,
    project_id: &str,
    acceptance: &crate::agent::acceptance::AcceptanceReport,
) -> Result<(), String> {
    let run_filter =
        "(trace_id=?1 OR trace_id IN (SELECT run_id FROM agent_dag_nodes WHERE root_run_id=?1))";
    let packs = crate::agent::tools::capabilities::select(goal)
        .iter()
        .map(|pack| pack.id)
        .collect::<Vec<_>>()
        .join(",");
    conn.execute(
        &format!("UPDATE tool_runs SET capability_pack=?2,
         model=COALESCE((SELECT model FROM agent_dag_nodes node WHERE node.run_id=tool_runs.trace_id),?3),project_id=?4,
         producer_version=?5,contribution_state=CASE WHEN status='ok' THEN
         'no_direct_acceptance_evidence' ELSE 'failed_or_blocked' END WHERE {run_filter}"),
        params![root_run_id, packs, model, project_id, env!("CARGO_PKG_VERSION")],
    ).map_err(|e| e.to_string())?;

    for digest in acceptance
        .criteria
        .iter()
        .flat_map(|criterion| &criterion.evidence)
        .filter_map(|label| label.rsplit_once('[')?.1.strip_suffix(']'))
    {
        conn.execute(
            &format!(
                "UPDATE tool_runs SET contribution_state='contributing'
             WHERE {run_filter} AND substr(evidence_digest,1,12)=?2"
            ),
            params![root_run_id, digest],
        )
        .map_err(|e| e.to_string())?;
    }

    let mut stmt = conn
        .prepare(&format!(
            "SELECT id,tool_name,
        COALESCE((SELECT goal FROM agent_dag_nodes node WHERE node.run_id=tool_runs.trace_id),?2)
        FROM tool_runs WHERE {run_filter}"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![root_run_id, goal], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);
    for (id, tool, run_goal) in rows {
        let selected = crate::agent::tools::capabilities::COMMON_TOOLS.contains(&tool.as_str())
            || crate::agent::tools::capabilities::select(&run_goal)
                .iter()
                .any(|pack| pack.tools.contains(&tool.as_str()));
        conn.execute(
            "UPDATE tool_runs SET selection_state=?1 WHERE id=?2",
            params![
                if selected {
                    "matched_capability_pack"
                } else {
                    "out_of_capability_pack"
                },
                id
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tool_runs(id TEXT,trace_id TEXT,tool_name TEXT,status TEXT,error_code TEXT,
             duration_ms INTEGER,created_at INTEGER,effect_kind TEXT,idempotency_key TEXT,retry_count INTEGER,
             cancellation_latency_ms INTEGER,cancel_requested_at INTEGER,cancel_observed_at INTEGER,
             contribution_state TEXT,selection_state TEXT,capability_pack TEXT,model TEXT,project_id TEXT,
             protocol_version INTEGER,producer_version TEXT,evidence_digest TEXT);
             CREATE TABLE agent_dag_nodes(root_run_id TEXT,run_id TEXT,goal TEXT,model TEXT);
             CREATE TABLE tool_protocol_versions(schema_version INTEGER,status TEXT,min_reader_version INTEGER,
             producer_version TEXT,compatibility TEXT,migration_notes TEXT);"
        ).unwrap();
        conn
    }

    #[test]
    fn summary_distinguishes_success_from_progress_and_failure_modes() {
        let conn = database();
        conn.execute_batch(
            "INSERT INTO tool_runs VALUES
             ('a','r','read_file','ok',NULL,10,100,'read','a',0,NULL,NULL,NULL,'contributing','matched_capability_pack','p','m','x',2,'2.0','aaa'),
             ('b','r','edit_file','ok',NULL,30,100,'write','b',1,NULL,NULL,NULL,'no_direct_acceptance_evidence','matched_capability_pack','p','m','x',2,'2.0','bbb'),
             ('c','r','read_file','error','TOOL_ARGUMENT_INVALID',20,100,'read','c',0,NULL,NULL,NULL,'failed_or_blocked','out_of_capability_pack','p','m','x',2,'2.0','ccc'),
             ('d','r','read_file','error','TOOL_TIMEOUT',40,100,'read','d',0,NULL,NULL,NULL,'failed_or_blocked','matched_capability_pack','p','m','x',2,'2.0','ddd'),
             ('e','r','read_file','cancelled','TOOL_CANCELLED',50,100,'read','e',0,25,1,26,'failed_or_blocked','matched_capability_pack','p','m','x',2,'2.0','eee');"
        ).unwrap();
        let metrics = summary(&conn, 0).unwrap();
        assert_eq!(metrics.total_calls, 5);
        assert_eq!(metrics.success_rate, 0.4);
        assert_eq!(metrics.argument_error_rate, 0.2);
        assert_eq!(metrics.timeout_rate, 0.2);
        assert_eq!(metrics.retry_rate, 0.2);
        assert_eq!(metrics.average_cancellation_latency_ms, Some(25.0));
        assert_eq!(metrics.contributing_success_rate, 0.5);
        assert_eq!(metrics.ineffective_call_rate, 0.5);
        assert_eq!(metrics.wrong_tool_selection_rate, 0.2);
        assert_eq!(
            breakdown(&conn, 0)
                .unwrap()
                .iter()
                .filter(|row| row.dimension == "tool")
                .count(),
            2
        );
    }

    #[test]
    fn final_acceptance_marks_contribution_dimensions_and_wrong_selection() {
        let conn = database();
        conn.execute(
            "INSERT INTO tool_runs VALUES('a','r','read_file','ok',NULL,10,100,'read','a',0,
             NULL,NULL,NULL,'unknown','unknown',NULL,NULL,NULL,2,NULL,'abcdef1234567890')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tool_runs VALUES('b','r','mystery_tool','ok',NULL,10,100,'read','b',0,
             NULL,NULL,NULL,'unknown','unknown',NULL,NULL,NULL,2,NULL,'bbbbbbbbbbbbbbbb')",
            [],
        )
        .unwrap();
        let acceptance = crate::agent::acceptance::AcceptanceReport {
            contract_version: 1,
            passed: true,
            criteria: vec![crate::agent::acceptance::AcceptanceCriterion {
                id: "understanding".into(),
                label: "understood".into(),
                required: true,
                passed: true,
                evidence: vec!["#1 read_file project [abcdef123456]".into()],
            }],
            blockers: Vec::new(),
            evidence_count: 1,
        };
        annotate_run_outcomes(
            &conn,
            "r",
            "阅读并理解项目",
            "model-a",
            "project-a",
            &acceptance,
        )
        .unwrap();
        let first: (String, String, String, String, String) = conn.query_row(
            "SELECT contribution_state,selection_state,capability_pack,model,project_id FROM tool_runs WHERE id='a'",
            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).unwrap();
        assert_eq!(first.0, "contributing");
        assert_eq!(first.1, "matched_capability_pack");
        assert!(first.2.contains("project_understanding"));
        assert_eq!(first.3, "model-a");
        assert_eq!(first.4, "project-a");
        let second: (String, String) = conn
            .query_row(
                "SELECT contribution_state,selection_state FROM tool_runs WHERE id='b'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            second,
            (
                "no_direct_acceptance_evidence".into(),
                "out_of_capability_pack".into()
            )
        );
    }

    #[test]
    fn attempt_metrics_persist_retry_and_cancel_latency() {
        let conn = database();
        conn.execute(
            "INSERT INTO tool_runs VALUES('c','r','read_file','cancelled','TOOL_CANCELLED',10,100,'read','c',0,
             NULL,NULL,NULL,'unknown','unknown',NULL,NULL,NULL,2,NULL,'c')", [],
        ).unwrap();
        let requested = chrono::Utc::now().timestamp_millis() - 20;
        record_attempt_metrics(&conn, "c", 2, Some(requested)).unwrap();
        let row: (i64, Option<i64>) = conn
            .query_row(
                "SELECT retry_count,cancellation_latency_ms FROM tool_runs WHERE id='c'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, 2);
        assert!(row.1.is_some_and(|latency| latency >= 20));
    }
}
