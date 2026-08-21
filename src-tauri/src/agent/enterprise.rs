//! 企业治理真源：SLO、告警、审计与配额使用量。所有写入均在本地 SQLite 完成，
//! 不依赖遥测服务；后续可通过只读接口安全导出。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentAlert {
    pub alert_id: String,
    pub run_id: Option<String>,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub state: String,
    pub details_json: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    pub audit_id: String,
    pub run_id: Option<String>,
    pub conversation_id: Option<String>,
    pub actor: String,
    pub action: String,
    pub resource: String,
    pub outcome: String,
    pub details_json: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct QuotaUsage {
    pub tenant_id: String,
    pub period: String,
    pub runs: i64,
    pub tool_calls: i64,
    pub failed_tools: i64,
    pub duration_ms: i64,
    pub cost_cny: f64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SloPolicy {
    pub policy_id: String,
    pub name: String,
    pub enabled: bool,
    pub acceptance_target: f64,
    pub recovery_target: f64,
    pub evidence_target: f64,
    pub max_duration_ms: i64,
    pub max_cost_cny: Option<f64>,
    pub updated_at: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn period() -> String {
    chrono::Utc::now().format("%Y-%m").to_string()
}

pub fn audit(
    conn: &Connection,
    run_id: Option<&str>,
    conversation_id: Option<&str>,
    actor: &str,
    action: &str,
    resource: &str,
    outcome: &str,
    details: &serde_json::Value,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO agent_audit_events
         (audit_id,tenant_id,run_id,conversation_id,actor,action,resource,outcome,details_json,created_at)
         VALUES (?1,'local',?2,?3,?4,?5,?6,?7,?8,?9)",
        params![uuid::Uuid::new_v4().to_string(),run_id,conversation_id,actor,action,resource,
            outcome,details.to_string(),now_ms()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn record_run_started(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
) -> Result<(), String> {
    increment_quota(conn, 1, 0, 0, 0)?;
    audit(
        conn,
        Some(run_id),
        Some(conversation_id),
        "user",
        "run.start",
        "agent_run",
        "accepted",
        &serde_json::json!({}),
    )
}

pub fn record_tool(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    tool: &str,
    status: &str,
    digest: &str,
) -> Result<(), String> {
    increment_quota(conn, 0, 1, i64::from(status != "ok"), 0)?;
    audit(
        conn,
        Some(run_id),
        Some(conversation_id),
        "agent",
        "tool.execute",
        tool,
        if status == "ok" { "success" } else { "failure" },
        &serde_json::json!({"status":status,"evidence_digest":digest}),
    )
}

pub fn record_cost(conn: &Connection, run_id: Option<&str>, cost_cny: f64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO agent_quota_usage(tenant_id,period,runs,tool_calls,failed_tools,duration_ms,cost_cny,updated_at)
         VALUES ('local',?1,0,0,0,0,?2,?3)
         ON CONFLICT(tenant_id,period) DO UPDATE SET cost_cny=cost_cny+excluded.cost_cny,updated_at=excluded.updated_at",
        params![period(),cost_cny.max(0.0),now_ms()],
    ).map_err(|e|e.to_string())?;
    if let Some(run_id) = run_id {
        let policy: Option<(String,f64)> = conn.query_row(
            "SELECT policy_id,max_cost_cny FROM agent_slo_policies WHERE tenant_id='local' AND enabled=1 AND max_cost_cny IS NOT NULL ORDER BY updated_at DESC LIMIT 1",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).ok();
        if let Some((policy_id, limit)) = policy {
            if cost_cny > limit {
                create_alert(
                    conn,
                    Some(run_id),
                    Some(&policy_id),
                    "warning",
                    "SLO_COST_EXCEEDED",
                    "Agent task exceeded its cost SLO",
                    &serde_json::json!({"cost_cny":cost_cny,"limit_cny":limit}),
                )?;
            }
        }
    }
    Ok(())
}

pub fn record_run_finished(
    conn: &Connection,
    run_id: &str,
    conversation_id: &str,
    state: &str,
    duration_ms: i64,
) -> Result<(), String> {
    increment_quota(conn, 0, 0, 0, duration_ms.max(0))?;
    audit(
        conn,
        Some(run_id),
        Some(conversation_id),
        "kernel",
        "run.finish",
        "agent_run",
        state,
        &serde_json::json!({"duration_ms":duration_ms}),
    )?;
    evaluate_run_slo(conn, run_id, duration_ms)
}

fn increment_quota(
    conn: &Connection,
    runs: i64,
    tools: i64,
    failed: i64,
    duration_ms: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO agent_quota_usage(tenant_id,period,runs,tool_calls,failed_tools,duration_ms,cost_cny,updated_at)
         VALUES ('local',?1,?2,?3,?4,?5,0,?6)
         ON CONFLICT(tenant_id,period) DO UPDATE SET runs=runs+excluded.runs,
         tool_calls=tool_calls+excluded.tool_calls,failed_tools=failed_tools+excluded.failed_tools,
         duration_ms=duration_ms+excluded.duration_ms,updated_at=excluded.updated_at",
        params![period(),runs,tools,failed,duration_ms,now_ms()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn evaluate_run_slo(conn: &Connection, run_id: &str, duration_ms: i64) -> Result<(), String> {
    let policy = conn.query_row(
        "SELECT policy_id,max_duration_ms FROM agent_slo_policies WHERE tenant_id='local' AND enabled=1 ORDER BY updated_at DESC LIMIT 1",
        [], |row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?)),
    ).ok();
    let Some((policy_id, max_duration)) = policy else {
        return Ok(());
    };
    if duration_ms > max_duration {
        create_alert(
            conn,
            Some(run_id),
            Some(&policy_id),
            "warning",
            "SLO_DURATION_EXCEEDED",
            "Agent task exceeded its duration SLO",
            &serde_json::json!({"duration_ms":duration_ms,"limit_ms":max_duration}),
        )?;
    }
    let run: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT state,acceptance_json FROM agent_runs WHERE run_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    if run.as_ref().is_some_and(|(state, _)| state == "completed")
        && run.as_ref().and_then(|(_, acceptance)| {
            acceptance
                .as_deref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .and_then(|value| value["passed"].as_bool())
        }) == Some(false)
    {
        create_alert(
            conn,
            Some(run_id),
            Some(&policy_id),
            "critical",
            "FALSE_COMPLETION_BLOCKED",
            "A terminal run failed evidence-based acceptance",
            &serde_json::json!({}),
        )?;
    }
    Ok(())
}

pub fn evaluate_window_slo(conn: &Connection, since_ms: i64) -> Result<(), String> {
    let policy = conn.query_row(
        "SELECT policy_id,acceptance_target,recovery_target,evidence_target FROM agent_slo_policies WHERE tenant_id='local' AND enabled=1 ORDER BY updated_at DESC LIMIT 1",
        [], |row| Ok((row.get::<_,String>(0)?,row.get::<_,f64>(1)?,row.get::<_,f64>(2)?,row.get::<_,f64>(3)?)),
    ).ok();
    let Some((policy_id, acceptance_target, recovery_target, evidence_target)) = policy else {
        return Ok(());
    };
    let (total, accepted, recovered, recovered_ok) = conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(CASE WHEN json_extract(acceptance_json,'$.passed')=1 THEN 1 ELSE 0 END),0),
         COALESCE(SUM(CASE WHEN recovery_mode='resume' THEN 1 ELSE 0 END),0),
         COALESCE(SUM(CASE WHEN recovery_mode='resume' AND state='completed' AND json_extract(acceptance_json,'$.passed')=1 THEN 1 ELSE 0 END),0)
         FROM agent_runs WHERE parent_run_id IS NULL AND started_at>=?1",
        [since_ms], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?)),
    ).unwrap_or((0,0,0,0));
    let (tools, structured) = conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(CASE WHEN protocol_version>=2 AND structured_result_json IS NOT NULL THEN 1 ELSE 0 END),0)
         FROM tool_runs WHERE created_at>=?1 AND status IN ('ok','error','blocked','cancelled','interrupted')",
        [since_ms/1000], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)),
    ).unwrap_or((0,0));
    if total >= 5 && accepted as f64 / (total as f64) < acceptance_target {
        alert_once(
            conn,
            &policy_id,
            "SLO_ACCEPTANCE_BREACH",
            "critical",
            "Agent acceptance rate is below SLO",
            serde_json::json!({"accepted":accepted,"total":total,"target":acceptance_target}),
        )?;
    }
    if recovered >= 3 && recovered_ok as f64 / (recovered as f64) < recovery_target {
        alert_once(
            conn,
            &policy_id,
            "SLO_RECOVERY_BREACH",
            "warning",
            "Agent recovery rate is below SLO",
            serde_json::json!({"recovered_ok":recovered_ok,"recovered":recovered,"target":recovery_target}),
        )?;
    }
    if tools >= 5 && structured as f64 / (tools as f64) < evidence_target {
        alert_once(
            conn,
            &policy_id,
            "SLO_EVIDENCE_BREACH",
            "warning",
            "Structured tool evidence coverage is below SLO",
            serde_json::json!({"structured":structured,"tools":tools,"target":evidence_target}),
        )?;
    }
    Ok(())
}

fn alert_once(
    conn: &Connection,
    policy_id: &str,
    code: &str,
    severity: &str,
    message: &str,
    details: serde_json::Value,
) -> Result<(), String> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_alerts WHERE tenant_id='local' AND policy_id=?1 AND code=?2 AND state='open')",
        params![policy_id,code], |row| row.get::<_,bool>(0),
    ).unwrap_or(false);
    if !exists {
        create_alert(
            conn,
            None,
            Some(policy_id),
            severity,
            code,
            message,
            &details,
        )?;
    }
    Ok(())
}

pub fn create_alert(
    conn: &Connection,
    run_id: Option<&str>,
    policy_id: Option<&str>,
    severity: &str,
    code: &str,
    message: &str,
    details: &serde_json::Value,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO agent_alerts(alert_id,tenant_id,run_id,policy_id,severity,code,message,state,details_json,created_at)
         VALUES (?1,'local',?2,?3,?4,?5,?6,'open',?7,?8)",
        params![uuid::Uuid::new_v4().to_string(),run_id,policy_id,severity,code,message,details.to_string(),now_ms()],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_alerts(conn: &Connection, limit: usize) -> Result<Vec<AgentAlert>, String> {
    let mut stmt = conn.prepare("SELECT alert_id,run_id,severity,code,message,state,details_json,created_at,resolved_at FROM agent_alerts ORDER BY CASE state WHEN 'open' THEN 0 ELSE 1 END,created_at DESC LIMIT ?1").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit.clamp(1, 500) as i64], |row| {
            Ok(AgentAlert {
                alert_id: row.get(0)?,
                run_id: row.get(1)?,
                severity: row.get(2)?,
                code: row.get(3)?,
                message: row.get(4)?,
                state: row.get(5)?,
                details_json: row.get(6)?,
                created_at: row.get(7)?,
                resolved_at: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn list_audit(
    conn: &Connection,
    run_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AuditEvent>, String> {
    let mut stmt = conn.prepare("SELECT audit_id,run_id,conversation_id,actor,action,resource,outcome,details_json,created_at FROM agent_audit_events WHERE (?1 IS NULL OR run_id=?1) ORDER BY created_at DESC LIMIT ?2").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![run_id, limit.clamp(1, 1000) as i64], |row| {
            Ok(AuditEvent {
                audit_id: row.get(0)?,
                run_id: row.get(1)?,
                conversation_id: row.get(2)?,
                actor: row.get(3)?,
                action: row.get(4)?,
                resource: row.get(5)?,
                outcome: row.get(6)?,
                details_json: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn quota(conn: &Connection) -> Result<QuotaUsage, String> {
    conn.query_row("SELECT tenant_id,period,runs,tool_calls,failed_tools,duration_ms,cost_cny,updated_at FROM agent_quota_usage WHERE tenant_id='local' AND period=?1", [period()], |row| Ok(QuotaUsage { tenant_id:row.get(0)?,period:row.get(1)?,runs:row.get(2)?,tool_calls:row.get(3)?,failed_tools:row.get(4)?,duration_ms:row.get(5)?,cost_cny:row.get(6)?,updated_at:row.get(7)? })).or_else(|_| Ok(QuotaUsage { tenant_id:"local".into(),period:period(),..Default::default() })).map_err(|e: rusqlite::Error| e.to_string())
}

pub fn get_policy(conn: &Connection) -> Result<Option<SloPolicy>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT policy_id,name,enabled,acceptance_target,recovery_target,evidence_target,max_duration_ms,max_cost_cny,updated_at FROM agent_slo_policies WHERE tenant_id='local' ORDER BY updated_at DESC LIMIT 1",[],|row| Ok(SloPolicy { policy_id:row.get(0)?,name:row.get(1)?,enabled:row.get(2)?,acceptance_target:row.get(3)?,recovery_target:row.get(4)?,evidence_target:row.get(5)?,max_duration_ms:row.get(6)?,max_cost_cny:row.get(7)?,updated_at:row.get(8)? })).optional().map_err(|e|e.to_string())
}

pub fn update_policy(conn: &Connection, policy: &SloPolicy) -> Result<(), String> {
    if !(0.0..=1.0).contains(&policy.acceptance_target)
        || !(0.0..=1.0).contains(&policy.recovery_target)
        || !(0.0..=1.0).contains(&policy.evidence_target)
    {
        return Err("SLO targets must be between 0 and 1".into());
    }
    if policy.max_duration_ms < 10_000 {
        return Err("SLO duration must be at least 10 seconds".into());
    }
    conn.execute("UPDATE agent_slo_policies SET name=?1,enabled=?2,acceptance_target=?3,recovery_target=?4,evidence_target=?5,max_duration_ms=?6,max_cost_cny=?7,updated_at=?8 WHERE policy_id=?9 AND tenant_id='local'",params![policy.name,policy.enabled,policy.acceptance_target,policy.recovery_target,policy.evidence_target,policy.max_duration_ms,policy.max_cost_cny,now_ms(),policy.policy_id]).map_err(|e|e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audit_quota_and_slo_alert_are_durable() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_runs(run_id TEXT PRIMARY KEY,state TEXT,acceptance_json TEXT); INSERT INTO agent_runs VALUES('r','completed','{\"passed\":false}'); CREATE TABLE agent_slo_policies(policy_id TEXT PRIMARY KEY,tenant_id TEXT,name TEXT,enabled INTEGER,acceptance_target REAL,recovery_target REAL,evidence_target REAL,max_duration_ms INTEGER,max_cost_cny REAL,created_at INTEGER,updated_at INTEGER); INSERT INTO agent_slo_policies VALUES('p','local','p',1,.95,.9,.95,100,NULL,0,0); CREATE TABLE agent_alerts(alert_id TEXT PRIMARY KEY,tenant_id TEXT,run_id TEXT,policy_id TEXT,severity TEXT,code TEXT,message TEXT,state TEXT,details_json TEXT,created_at INTEGER,resolved_at INTEGER); CREATE TABLE agent_audit_events(audit_id TEXT PRIMARY KEY,tenant_id TEXT,run_id TEXT,conversation_id TEXT,actor TEXT,action TEXT,resource TEXT,outcome TEXT,details_json TEXT,created_at INTEGER); CREATE TABLE agent_quota_usage(tenant_id TEXT,period TEXT,runs INTEGER,tool_calls INTEGER,failed_tools INTEGER,duration_ms INTEGER,cost_cny REAL,updated_at INTEGER,PRIMARY KEY(tenant_id,period));").unwrap();
        record_run_started(&conn, "r", "c").unwrap();
        record_tool(&conn, "r", "c", "read_file", "ok", "d").unwrap();
        record_run_finished(&conn, "r", "c", "completed", 200).unwrap();
        assert_eq!(quota(&conn).unwrap().runs, 1);
        assert_eq!(quota(&conn).unwrap().tool_calls, 1);
        assert_eq!(list_alerts(&conn, 10).unwrap().len(), 2);
        assert_eq!(list_audit(&conn, Some("r"), 10).unwrap().len(), 3);
    }
}
