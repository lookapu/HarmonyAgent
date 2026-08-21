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
    pub attempt: i64,
    pub max_attempts: i64,
    pub next_attempt_at: Option<i64>,
    pub condition_json: String,
    pub failure_policy: String,
    pub concurrency_key: Option<String>,
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
    let cancelled = error
        .is_some_and(|value| value.contains("停止") || value.to_lowercase().contains("cancel"));
    let state = if passed {
        "completed"
    } else if cancelled {
        "cancelled"
    } else {
        "failed"
    };
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
    if !passed {
        let policy: String = conn
            .query_row(
                "SELECT failure_policy FROM agent_dag_nodes WHERE node_id=?1",
                [node_id],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "fail_fast".into());
        if policy == "fail_fast" {
            let _ = cancel_descendants(
                conn,
                root_run_id,
                node_id,
                error.unwrap_or("upstream node failed"),
            );
        }
    }
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

pub fn recover_orphaned_nodes(conn: &Connection) -> Result<usize, String> {
    let now = now_ms();
    conn.execute(
        "UPDATE agent_dag_nodes SET state=CASE WHEN attempt<max_attempts THEN 'retry_wait' ELSE 'failed' END,
         next_attempt_at=CASE WHEN attempt<max_attempts THEN ?1 ELSE NULL END,
         output_summary=COALESCE(output_summary,'应用重启，节点等待局部恢复'),updated_at=?1
         WHERE state='running' AND parent_node_id IS NOT NULL AND EXISTS(
           SELECT 1 FROM agent_task_queue q WHERE q.state='recovery_required'
           AND (q.run_id=agent_dag_nodes.run_id OR q.run_id=agent_dag_nodes.root_run_id))",
        [now],
    ).map_err(|e|e.to_string())
}

pub fn list_nodes(conn: &Connection, root_run_id: &str) -> Result<Vec<DagNode>, String> {
    let mut stmt = conn.prepare(
        "SELECT node_id,root_run_id,run_id,parent_node_id,name,goal,model,state,acceptance_json,
                output_summary,created_at,updated_at,finished_at,attempt,max_attempts,next_attempt_at,
                condition_json,failure_policy,concurrency_key
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
                attempt: row.get(13)?,
                max_attempts: row.get(14)?,
                next_attempt_at: row.get(15)?,
                condition_json: row.get(16)?,
                failure_policy: row.get(17)?,
                concurrency_key: row.get(18)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn add_dependency(
    conn: &Connection,
    root_run_id: &str,
    from_node_id: &str,
    to_node_id: &str,
    required: bool,
    condition: &serde_json::Value,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO agent_dag_edges(root_run_id,from_node_id,to_node_id,edge_kind,created_at,condition_json,required)
         VALUES (?1,?2,?3,'depends_on',?4,?5,?6)
         ON CONFLICT(root_run_id,from_node_id,to_node_id) DO UPDATE SET condition_json=excluded.condition_json,required=excluded.required",
        params![root_run_id,from_node_id,to_node_id,now_ms(),condition.to_string(),required],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// 返回依赖均已成功（或非必需依赖已终止）的 pending/retry_wait 节点。
pub fn runnable_nodes(
    conn: &Connection,
    root_run_id: &str,
    limit: usize,
) -> Result<Vec<DagNode>, String> {
    let now = now_ms();
    let mut stmt = conn.prepare(
        "SELECT n.node_id,n.root_run_id,n.run_id,n.parent_node_id,n.name,n.goal,n.model,n.state,
         n.acceptance_json,n.output_summary,n.created_at,n.updated_at,n.finished_at,n.attempt,n.max_attempts,
         n.next_attempt_at,n.condition_json,n.failure_policy,n.concurrency_key
         FROM agent_dag_nodes n WHERE n.root_run_id=?1 AND n.state IN ('pending','retry_wait')
         AND (n.next_attempt_at IS NULL OR n.next_attempt_at<=?2)
         AND NOT EXISTS (SELECT 1 FROM agent_dag_edges e JOIN agent_dag_nodes upstream ON upstream.node_id=e.from_node_id
           WHERE e.root_run_id=n.root_run_id AND e.to_node_id=n.node_id AND e.required=1 AND upstream.state!='completed')
         AND (n.concurrency_key IS NULL OR NOT EXISTS (SELECT 1 FROM agent_dag_nodes active
           WHERE active.root_run_id=n.root_run_id AND active.concurrency_key=n.concurrency_key AND active.state='running'))
         ORDER BY n.created_at,n.node_id LIMIT ?3"
    ).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(
            params![root_run_id, now, limit.clamp(1, 100) as i64],
            row_to_node,
        )
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn fail_or_retry_node(
    conn: &Connection,
    node_id: &str,
    error: &str,
    delay_ms: i64,
) -> Result<String, String> {
    let now = now_ms();
    conn.execute(
        "UPDATE agent_dag_nodes SET state=CASE WHEN attempt<max_attempts THEN 'retry_wait' ELSE 'failed' END,
         attempt=CASE WHEN attempt<max_attempts THEN attempt+1 ELSE attempt END,
         next_attempt_at=CASE WHEN attempt<max_attempts THEN ?1 ELSE NULL END,
         output_summary=?2,updated_at=?3,finished_at=CASE WHEN attempt<max_attempts THEN NULL ELSE ?3 END
         WHERE node_id=?4 AND state IN ('running','pending')",
        params![now.saturating_add(delay_ms.max(0)),error.chars().take(1000).collect::<String>(),now,node_id],
    ).map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT state FROM agent_dag_nodes WHERE node_id=?1",
        [node_id],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// 递归取消所有尚未终止的下游节点；已完成节点保持不可变。
pub fn cancel_descendants(
    conn: &Connection,
    root_run_id: &str,
    node_id: &str,
    reason: &str,
) -> Result<usize, String> {
    conn.execute(
        "WITH RECURSIVE descendants(node_id) AS (
           SELECT to_node_id FROM agent_dag_edges WHERE root_run_id=?1 AND from_node_id=?2
           UNION SELECT e.to_node_id FROM agent_dag_edges e JOIN descendants d ON e.from_node_id=d.node_id WHERE e.root_run_id=?1)
         UPDATE agent_dag_nodes SET state='cancelled',output_summary=?3,updated_at=?4,finished_at=?4
         WHERE node_id IN descendants AND state NOT IN ('completed','failed','cancelled')",
        params![root_run_id,node_id,reason,now_ms()],
    ).map_err(|e| e.to_string())
}

fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<DagNode> {
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
        attempt: row.get(13)?,
        max_attempts: row.get(14)?,
        next_attempt_at: row.get(15)?,
        condition_json: row.get(16)?,
        failure_policy: row.get(17)?,
        concurrency_key: row.get(18)?,
    })
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
            attempt: 1,
            max_attempts: 2,
            next_attempt_at: None,
            condition_json: "{}".into(),
            failure_policy: "fail_fast".into(),
            concurrency_key: None,
        };
        assert!(serde_json::to_string(&node).unwrap().contains("review"));
    }

    #[test]
    fn dependency_retry_and_cancel_flow() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_dag_nodes(node_id TEXT PRIMARY KEY,root_run_id TEXT,run_id TEXT,parent_node_id TEXT,name TEXT,goal TEXT,model TEXT,state TEXT,acceptance_json TEXT,output_summary TEXT,created_at INTEGER,updated_at INTEGER,finished_at INTEGER,attempt INTEGER DEFAULT 1,max_attempts INTEGER DEFAULT 2,next_attempt_at INTEGER,condition_json TEXT DEFAULT '{}',failure_policy TEXT DEFAULT 'fail_fast',concurrency_key TEXT); CREATE TABLE agent_dag_edges(root_run_id TEXT,from_node_id TEXT,to_node_id TEXT,edge_kind TEXT,created_at INTEGER,condition_json TEXT DEFAULT '{}',required INTEGER DEFAULT 1,PRIMARY KEY(root_run_id,from_node_id,to_node_id)); INSERT INTO agent_dag_nodes(node_id,root_run_id,run_id,name,goal,state,created_at,updated_at) VALUES ('a','r','ra','a','a','running',1,1),('b','r','rb','b','b','pending',2,2),('c','r','rc','c','c','pending',3,3);").unwrap();
        add_dependency(&conn, "r", "a", "b", true, &serde_json::json!({})).unwrap();
        add_dependency(&conn, "r", "b", "c", true, &serde_json::json!({})).unwrap();
        assert!(runnable_nodes(&conn, "r", 10).unwrap().is_empty());
        conn.execute(
            "UPDATE agent_dag_nodes SET state='completed' WHERE node_id='a'",
            [],
        )
        .unwrap();
        assert_eq!(runnable_nodes(&conn, "r", 10).unwrap()[0].node_id, "b");
        conn.execute(
            "UPDATE agent_dag_nodes SET state='running' WHERE node_id='b'",
            [],
        )
        .unwrap();
        assert_eq!(
            fail_or_retry_node(&conn, "b", "temporary", 0).unwrap(),
            "retry_wait"
        );
        assert_eq!(
            cancel_descendants(&conn, "r", "b", "upstream cancelled").unwrap(),
            1
        );
    }
}
