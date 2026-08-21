//! 可重复的 Agent 可靠性评测套件。场景与期望策略作为版本化 fixture 随仓库维护，
//! 本地、Windows CI、macOS CI 使用完全相同的质量阈值。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub const DEFAULT_RELIABILITY_THRESHOLD: f64 = 0.95;

#[derive(Clone, Debug, Deserialize)]
pub struct ReliabilityScenario {
    pub id: String,
    pub domain: String,
    pub expected: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalCaseResult {
    pub id: String,
    pub domain: String,
    pub expected: String,
    pub actual: String,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalRun {
    pub eval_run_id: String,
    pub suite: String,
    pub platform: String,
    pub passed: bool,
    pub total_cases: usize,
    pub passed_cases: usize,
    pub score: f64,
    pub threshold: f64,
    pub results: Vec<EvalCaseResult>,
    pub created_at: i64,
}

pub fn scenarios() -> Vec<ReliabilityScenario> {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/agent_reliability_scenarios.json"
    ))
    .unwrap_or_default()
}

pub fn run_suite(conn: Option<&Connection>, threshold: f64) -> Result<EvalRun, String> {
    let threshold = threshold.clamp(0.0, 1.0);
    let results = scenarios()
        .into_iter()
        .map(|scenario| {
            let actual = disposition_for_id(&scenario.id)
                .unwrap_or("unhandled")
                .to_string();
            EvalCaseResult {
                passed: actual == scenario.expected,
                id: scenario.id,
                domain: scenario.domain,
                expected: scenario.expected,
                actual,
            }
        })
        .collect::<Vec<_>>();
    let passed_cases = results.iter().filter(|result| result.passed).count();
    let score = if results.is_empty() {
        0.0
    } else {
        passed_cases as f64 / results.len() as f64
    };
    let created_at = chrono::Utc::now().timestamp_millis();
    let run = EvalRun {
        eval_run_id: uuid::Uuid::new_v4().to_string(),
        suite: "agent_reliability_v1".into(),
        platform: std::env::consts::OS.into(),
        passed: score >= threshold,
        total_cases: results.len(),
        passed_cases,
        score,
        threshold,
        results,
        created_at,
    };
    if let Some(conn) = conn {
        conn.execute(
            "INSERT INTO agent_eval_runs
             (eval_run_id,suite,platform,passed,total_cases,passed_cases,score,threshold,results_json,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![run.eval_run_id, run.suite, run.platform, run.passed, run.total_cases as i64,
                run.passed_cases as i64, run.score, run.threshold, serde_json::to_string(&run.results).unwrap_or_default(), run.created_at],
        ).map_err(|e| e.to_string())?;
    }
    Ok(run)
}

fn disposition_for_id(id: &str) -> Option<&'static str> {
    use crate::agent::governance::{reliability_disposition as disposition, FailureSignal::*};
    Some(disposition(match id {
        "stream_disconnect_before_delta" => StreamBeforeDelta,
        "stream_disconnect_after_delta" => StreamAfterDelta,
        "model_output_truncated" => ModelTruncated,
        "readonly_tool_timeout" => ReadTimeout,
        "write_tool_timeout" => WriteTimeout,
        "restart_with_prepared_effect" => RestartPreparedEffect,
        "approval_timeout" => ApprovalTimeout,
        "stale_terminal_event" => StaleTerminal,
        "completion_without_evidence" => MissingEvidence,
        "budget_exhaustion" => BudgetExhausted,
        "subagent_claim_without_tools" => SubagentMissingEvidence,
        _ => return None,
    }))
}

/// 调试/评测构建使用的显式故障点。发布构建永远返回 false，防止环境变量误伤用户任务。
pub fn fault_enabled(point: &str) -> bool {
    cfg!(debug_assertions) && std::env::var("HARMONY_AGENT_FAULT").ok().as_deref() == Some(point)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reliability_gate() {
        let run = run_suite(None, DEFAULT_RELIABILITY_THRESHOLD).unwrap();
        assert!(run.passed, "score={} results={:?}", run.score, run.results);
        assert_eq!(run.score, 1.0);
        assert!(
            run.results
                .iter()
                .map(|item| &item.domain)
                .collect::<std::collections::HashSet<_>>()
                .len()
                >= 8
        );
    }

    #[test]
    fn unknown_scenario_fails_closed() {
        assert_eq!(disposition_for_id("new_unhandled_failure"), None);
    }
}
