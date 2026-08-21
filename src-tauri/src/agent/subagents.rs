//! 子 Agent 委派协议与运行登记。权威执行状态、契约和结果由 Durable Run/DAG 持久化；
//! OnceLock 只保留供 list_agents 快速查看的最近 50 条非权威摘要。

use std::sync::{Mutex, OnceLock};

pub const SUB_AGENT_PROTOCOL_VERSION: u32 = 2;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DelegatedTaskContractV2 {
    pub version: u32,
    pub goal_contract: crate::agent::acceptance::GoalContract,
    pub context_refs: Vec<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub max_spawn_depth: usize,
    pub parent_transcript_included: bool,
    pub result_schema: String,
}

impl DelegatedTaskContractV2 {
    pub fn new(
        goal_contract: crate::agent::acceptance::GoalContract,
        context_refs: &[String],
        allowed_tools: Option<Vec<String>>,
        max_spawn_depth: usize,
    ) -> Self {
        Self {
            version: SUB_AGENT_PROTOCOL_VERSION,
            goal_contract,
            context_refs: context_refs.iter().take(32).map(|item| item.chars().take(1024).collect()).collect(),
            allowed_tools,
            max_spawn_depth,
            parent_transcript_included: false,
            result_schema: "SubAgentResultV2".into(),
        }
    }

    pub fn directive(&self) -> String {
        format!(
            "委派协议 v{}：你只能使用显式任务契约、项目规则和以下上下文引用，不包含父会话全文：{}。最终结果必须由运行内核转换为 SubAgentResultV2，不能用自由文本替代验收证据。",
            self.version,
            if self.context_refs.is_empty() { "（无）".into() } else { self.context_refs.join("、") },
        )
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SubAgentArtifactRef {
    pub kind: String,
    pub uri: String,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SubAgentResultV2 {
    pub version: u32,
    pub name: String,
    pub run_id: String,
    pub status: String,
    pub summary: String,
    pub acceptance: crate::agent::acceptance::AcceptanceReport,
    pub artifacts: Vec<SubAgentArtifactRef>,
    pub evidence_refs: Vec<String>,
    pub blockers: Vec<String>,
    pub error: Option<String>,
}

pub fn build_result(
    conn: &rusqlite::Connection,
    name: &str,
    run_id: &str,
    report: &crate::agent::acceptance::AcceptanceReport,
    output: &str,
    error: Option<&str>,
) -> SubAgentResultV2 {
    let artifacts = conn.prepare(
        "SELECT artifact_kind,uri,digest FROM conversation_context_artifacts
         WHERE run_id=?1 AND valid=1 ORDER BY updated_at DESC LIMIT 50",
    ).and_then(|mut stmt| stmt.query_map([run_id], |row| Ok(SubAgentArtifactRef {
        kind: row.get(0)?, uri: row.get(1)?, digest: row.get(2)?,
    }))?.collect::<Result<Vec<_>, _>>()).unwrap_or_default();
    let mut evidence_refs = report.criteria.iter()
        .flat_map(|criterion| criterion.evidence.iter().cloned())
        .collect::<Vec<_>>();
    evidence_refs.sort();
    evidence_refs.dedup();
    evidence_refs.truncate(100);
    SubAgentResultV2 {
        version: SUB_AGENT_PROTOCOL_VERSION,
        name: name.to_string(),
        run_id: run_id.to_string(),
        status: if error.is_some() { "error".into() } else if report.passed { "completed".into() } else { "failed_acceptance".into() },
        summary: output.chars().take(2000).collect(),
        acceptance: report.clone(),
        artifacts,
        evidence_refs,
        blockers: report.blockers.clone(),
        error: error.map(|value| value.chars().take(2000).collect()),
    }
}

pub fn aggregate_results(outputs: Vec<(String, Result<String, String>)>) -> String {
    let results = outputs.into_iter().map(|(name, result)| {
        let (status, raw) = match result {
            Ok(value) => ("completed", value),
            Err(value) => ("error", value),
        };
        serde_json::from_str::<serde_json::Value>(&raw).unwrap_or_else(|_| serde_json::json!({
            "version": SUB_AGENT_PROTOCOL_VERSION,
            "name": name,
            "status": status,
            "summary": "",
            "acceptance": null,
            "artifacts": [],
            "evidence_refs": [],
            "blockers": [],
            "error": raw.chars().take(2000).collect::<String>(),
        }))
    }).collect::<Vec<_>>();
    serde_json::json!({
        "protocol_version": SUB_AGENT_PROTOCOL_VERSION,
        "result_schema": "SubAgentResultV2[]",
        "total": results.len(),
        "results": results,
    }).to_string()
}

/// 一条子 Agent 运行记录
#[derive(Clone, serde::Serialize)]
pub struct SubAgentRecord {
    /// 任务名（spawn_agents 的 name 参数）
    pub name: String,
    /// 实际使用的模型
    pub model: String,
    /// 开始时间（HH:MM:SS，本地时区）
    pub started_at: String,
    /// done | error | skipped（skipped=用户停止后未执行）
    pub status: String,
    /// 耗时毫秒（skipped 为 0）
    pub elapsed_ms: i64,
    /// 输出尾部摘要（最多 200 字符）
    pub output_tail: String,
}

static REGISTRY: OnceLock<Mutex<Vec<SubAgentRecord>>> = OnceLock::new();

fn table() -> &'static Mutex<Vec<SubAgentRecord>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// 追加一条运行记录（只保留最近 50 条，超出丢弃最旧）
pub fn record(rec: SubAgentRecord) {
    if let Ok(mut v) = table().lock() {
        v.push(rec);
        if v.len() > 50 {
            let excess = v.len() - 50;
            v.drain(..excess);
        }
    }
}

/// 运行记录快照（新 → 旧）
pub fn snapshot() -> Vec<SubAgentRecord> {
    table()
        .lock()
        .map(|v| v.iter().rev().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegated_contract_is_bounded_and_excludes_parent_transcript() {
        let refs = (0..40).map(|index| format!("src/{index}.ets")).collect::<Vec<_>>();
        let contract = DelegatedTaskContractV2::new(
            crate::agent::acceptance::GoalContract::compile("检查并测试"),
            &refs,
            Some(vec!["read_file".into(), "run_tests".into()]),
            0,
        );
        assert_eq!(contract.context_refs.len(), 32);
        assert!(!contract.parent_transcript_included);
        assert_eq!(contract.result_schema, "SubAgentResultV2");
    }

    #[test]
    fn structured_result_carries_acceptance_artifacts_and_errors() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE conversation_context_artifacts(
               id TEXT,run_id TEXT,artifact_kind TEXT,uri TEXT,digest TEXT,valid INTEGER,updated_at INTEGER);
             INSERT INTO conversation_context_artifacts VALUES('a','child','file','src/a.ets','sha',1,1);",
        ).unwrap();
        let report = crate::agent::acceptance::AcceptanceReport {
            contract_version: 1,
            passed: false,
            criteria: vec![crate::agent::acceptance::AcceptanceCriterion {
                id: "tests".into(), label: "测试通过".into(), required: true,
                passed: false, evidence: vec!["tool:test".into()],
            }],
            blockers: vec!["测试通过".into()],
            evidence_count: 1,
        };
        let result = build_result(&conn, "review", "child", &report, "partial", Some("test failed"));
        assert_eq!(result.status, "error");
        assert_eq!(result.artifacts[0].uri, "src/a.ets");
        assert_eq!(result.evidence_refs, vec!["tool:test"]);
        assert_eq!(result.blockers, vec!["测试通过"]);
        assert_eq!(result.error.as_deref(), Some("test failed"));
    }
    #[test]
    fn aggregate_results_is_a_single_machine_readable_envelope() {
        let encoded = aggregate_results(vec![
            ("ok".into(), Ok(r#"{"version":2,"name":"ok","status":"completed"}"#.into())),
            ("bad".into(), Err("worker unavailable".into())),
        ]);
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["total"], 2);
        assert_eq!(value["results"][0]["status"], "completed");
        assert_eq!(value["results"][1]["status"], "error");
        assert_eq!(value["results"][1]["error"], "worker unavailable");
    }
}
