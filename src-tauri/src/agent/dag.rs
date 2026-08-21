//! 多 Agent 持久化执行图。每个子 Agent 拥有独立 run_id、目标契约、预算、工具证据和
//! 终态；父 Run 只汇聚已通过验收的节点结果。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DagNode {
    pub node_id: String,
    pub root_run_id: String,
    pub run_id: String,
    pub parent_node_id: Option<String>,
    pub name: String,
    pub goal: String,
    pub model: Option<String>,
    pub state: String,
    pub acceptance_json: Option<String>,
    pub output_summary: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn register_root(
    conn: &Connection,
    run_id: &str,
    goal: &str,
    model: Option<&str>,
) -> Result<String, String> {
    let node_id = format!("root:{run_id}");
    let now = now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO agent_dag_nodes
         (node_id,root_run_id,run_id,parent_node_id,name,goal,model,state,created_at,updated_at)
         VALUES (?1,?2,?2,NULL,'root',?3,?4,'running',?5,?5)",
        params![node_id, run_id, goal, model, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE agent_runs SET root_run_id=?1,dag_node_id=?2 WHERE run_id=?1",
        params![run_id, node_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(node_id)
}

#[allow(clippy::too_many_arguments)]
pub fn begin_child(
    conn: &Connection,
    root_run_id: &str,
    parent_node_id: &str,
    conversation_id: &str,
    name: &str,
    goal: &str,
    model: &str,
    contract: &crate::agent::acceptance::GoalContract,
    budget: &crate::agent::governance::ExecutionBudget,
) -> Result<(String, String), String> {
    let node_id = uuid::Uuid::new_v4().to_string();
    let run_id = uuid::Uuid::new_v4().to_string();
    let now = now_ms();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO agent_runs
         (run_id,conversation_id,goal,state,phase,attempt,last_event_seq,recovery_count,resume_policy,
          metadata_json,started_at,updated_at,recovery_mode,goal_contract_json,remediation_count,
          heartbeat_at,lease_expires_at,root_run_id,dag_node_id,budget_json)
         VALUES (?1,?2,?3,'delegated_running','orchestrating',1,0,0,'continue','{}',?4,?4,'fresh',?5,0,?4,?6,?7,?8,?9)",
        params![run_id, conversation_id, goal, now, serde_json::to_string(contract).unwrap_or_default(),
            now.saturating_add(budget.lease_ms), root_run_id, node_id, serde_json::to_string(budget).unwrap_or_default()],
    ).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO agent_dag_nodes
         (node_id,root_run_id,run_id,parent_node_id,name,goal,model,state,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,'running',?8,?8)",
        params![
            node_id,
            root_run_id,
            run_id,
            parent_node_id,
            name,
            goal,
            model,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR IGNORE INTO agent_dag_edges(root_run_id,from_node_id,to_node_id,edge_kind,created_at)
         VALUES (?1,?2,?3,'delegates',?4)",
        params![root_run_id, parent_node_id, node_id, now],
    ).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    let _ = crate::agent::runtime::append_event(
        conn,
        root_run_id,
        conversation_id,
        "dag.node_started",
        serde_json::json!({
            "node_id": node_id, "child_run_id": run_id, "name": name, "model": model,
        }),
    );
    let _ = crate::agent::runtime::append_event(
        conn,
        &run_id,
        conversation_id,
        "run.started",
        serde_json::json!({
            "root_run_id": root_run_id, "node_id": node_id, "goal": goal,
        }),
    );
    Ok((node_id, run_id))
}

#[allow(clippy::too_many_arguments)]
pub fn finish_child(
    conn: &Connection,
    root_run_id: &str,
    conversation_id: &str,
    node_id: &str,
    child_run_id: &str,
    report: &crate::agent::acceptance::AcceptanceReport,
    output: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let now = now_ms();
    let passed = error.is_none() && report.passed;
    let state = if passed { "completed" } else { "failed" };
    let acceptance = serde_json::to_string(report).unwrap_or_default();
    conn.execute(
        "UPDATE agent_dag_nodes SET state=?1,acceptance_json=?2,output_summary=?3,updated_at=?4,finished_at=?4
         WHERE node_id=?5 AND state='running'",
        params![state, acceptance, output.chars().take(1000).collect::<String>(), now, node_id],
    ).map_err(|e| e.to_string())?;
    crate::agent::runtime::set_acceptance(
        conn,
        child_run_id,
        &serde_json::to_value(report).unwrap_or_default(),
    )?;
    crate::agent::runtime::transition(conn, child_run_id, conversation_id, state, "done", error)?;
    let _ = crate::agent::runtime::append_event(
        conn,
        root_run_id,
        conversation_id,
        "dag.node_finished",
        serde_json::json!({
            "node_id": node_id, "child_run_id": child_run_id, "state": state, "acceptance_passed": report.passed,
        }),
    );
    Ok(())
}

pub fn finish_root(conn: &Connection, root_run_id: &str, state: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_dag_nodes SET state=?1,updated_at=?2,finished_at=?2
         WHERE root_run_id=?3 AND node_id=?4 AND state='running'",
        params![state, now_ms(), root_run_id, format!("root:{root_run_id}")],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_nodes(conn: &Connection, root_run_id: &str) -> Result<Vec<DagNode>, String> {
    let mut stmt = conn.prepare(
        "SELECT node_id,root_run_id,run_id,parent_node_id,name,goal,model,state,acceptance_json,
                output_summary,created_at,updated_at,finished_at
         FROM agent_dag_nodes WHERE root_run_id=?1 ORDER BY created_at,node_id",
    ).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([root_run_id], |row| {
            Ok(DagNode {
                node_id: row.get(0)?,
                root_run_id: row.get(1)?,
                run_id: row.get(2)?,
                parent_node_id: row.get(3)?,
                name: row.get(4)?,
                goal: row.get(5)?,
                model: row.get(6)?,
                state: row.get(7)?,
                acceptance_json: row.get(8)?,
                output_summary: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
                finished_at: row.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn evaluate_run(
    conn: &Connection,
    run_id: &str,
    contract: &crate::agent::acceptance::GoalContract,
) -> Result<crate::agent::acceptance::AcceptanceReport, String> {
    let mut stmt = conn.prepare(
        "SELECT tool_name,input_json,result_json,status FROM tool_runs WHERE trace_id=?1 ORDER BY created_at,id",
    ).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let evidence = rows
        .iter()
        .map(
            |(tool, args, output, status)| crate::agent::acceptance::ToolEvidence {
                tool,
                args,
                output,
                succeeded: status == "ok",
            },
        )
        .collect::<Vec<_>>();
    Ok(crate::agent::acceptance::evaluate_contract(
        contract, &evidence,
    ))
}

pub fn evaluate_root_with_children(
    conn: &Connection,
    root_run_id: &str,
    contract: &crate::agent::acceptance::GoalContract,
    primary: &[crate::agent::acceptance::ToolEvidence<'_>],
) -> Result<crate::agent::acceptance::AcceptanceReport, String> {
    let mut stmt = conn
        .prepare(
            "SELECT tr.tool_name,tr.input_json,tr.result_json,tr.status
         FROM agent_dag_nodes node JOIN tool_runs tr ON tr.trace_id=node.run_id
         WHERE node.root_run_id=?1 AND node.state='completed' AND node.run_id!=?1
         ORDER BY node.created_at,tr.created_at,tr.id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([root_run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut evidence = primary.to_vec();
    evidence.extend(rows.iter().map(|(tool, args, output, status)| {
        crate::agent::acceptance::ToolEvidence {
            tool,
            args,
            output,
            succeeded: status == "ok",
        }
    }));
    Ok(crate::agent::acceptance::evaluate_contract(
        contract, &evidence,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn node_shape_serializes_for_control_plane() {
        let node = DagNode {
            node_id: "n".into(),
            root_run_id: "r".into(),
            run_id: "c".into(),
            parent_node_id: Some("root:r".into()),
            name: "review".into(),
            goal: "inspect".into(),
            model: None,
            state: "completed".into(),
            acceptance_json: None,
            output_summary: None,
            created_at: 1,
            updated_at: 2,
            finished_at: Some(2),
        };
        assert!(serde_json::to_string(&node).unwrap().contains("review"));
    }
}
