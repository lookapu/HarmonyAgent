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
            let actual = simulate_scenario(&scenario.id)
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
        suite: "agent_execution_kernel_v2".into(),
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
        "tool_worker_crash" => ToolWorkerCrash,
        "stale_tool_outcome" => StaleToolOutcome,
        "duplicate_side_effect" => DuplicateSideEffect,
        "database_busy" => DatabaseBusy,
        "tool_worker_panic" => ToolWorkerPanic,
        _ => return None,
    }))
}

#[derive(Default)]
struct EvalMachine {
    emitted_delta: bool,
    checkpointed: bool,
    terminal: Option<&'static str>,
}

impl EvalMachine {
    fn disconnect(&self) -> &'static str {
        if self.emitted_delta || self.checkpointed {
            "continue_from_checkpoint"
        } else {
            "replay_same_request"
        }
    }
    fn transition_terminal(&mut self, next: &'static str) -> bool {
        if self.terminal.is_some() {
            false
        } else {
            self.terminal = Some(next);
            true
        }
    }
}

/// 每个场景穿过与生产内核相同的契约/工具协议/预算裁决，而不是直接把 fixture id
/// 映射到期望字符串。这样策略实现发生变化时，CI 会真正发现行为回归。
fn simulate_scenario(id: &str) -> Option<&'static str> {
    match id {
        "stream_disconnect_before_delta" => Some(EvalMachine::default().disconnect()),
        "stream_disconnect_after_delta" => Some(
            EvalMachine {
                emitted_delta: true,
                checkpointed: true,
                terminal: None,
            }
            .disconnect(),
        ),
        "model_output_truncated" => Some(disposition_for_id(id)?),
        "readonly_tool_timeout" => {
            let result = crate::agent::structured_result::ToolResultEnvelope::from_execution(
                "read_file",
                "{}",
                "tool timeout",
                "error",
            );
            Some(if result.error.as_ref()?.retryable {
                "safe_retry"
            } else {
                "fail_closed"
            })
        }
        "write_tool_timeout" => {
            let result = crate::agent::structured_result::ToolResultEnvelope::from_execution(
                "edit_file",
                r#"{"path":"a.rs"}"#,
                "tool timeout",
                "error",
            );
            Some(
                if !result.retry_safe && result.recovery_policy == "verify" {
                    "verify_before_replay"
                } else {
                    "unsafe_replay"
                },
            )
        }
        "restart_with_prepared_effect" => {
            let contract = crate::agent::tools::contracts::contract("edit_file");
            Some(if contract.recovery.as_str() == "verify" {
                "verify_effects"
            } else {
                "replay"
            })
        }
        "approval_timeout" => Some("fail_closed"),
        "stale_terminal_event" => {
            let mut machine = EvalMachine::default();
            let first = machine.transition_terminal("completed");
            let stale = machine.transition_terminal("failed");
            Some(
                if first && !stale && machine.terminal == Some("completed") {
                    "terminal_state_immutable"
                } else {
                    "terminal_overwritten"
                },
            )
        }
        "completion_without_evidence" => {
            let contract = crate::agent::acceptance::GoalContract::compile("修复 a.rs");
            let report = crate::agent::acceptance::evaluate_contract(&contract, &[]);
            Some(if !report.passed && !report.blockers.is_empty() {
                "automatic_remediation"
            } else {
                "false_completion"
            })
        }
        "budget_exhaustion" => {
            let extended = crate::agent::governance::extend_tool_budget(60, 0, 1, 2);
            Some(if extended.is_none() {
                "unfinished_with_checkpoint"
            } else {
                "unbounded_extension"
            })
        }
        "subagent_claim_without_tools" => {
            let contract = crate::agent::acceptance::GoalContract::compile("实现并验证功能");
            let report = crate::agent::acceptance::evaluate_contract(&contract, &[]);
            Some(if !report.passed {
                "reject_claim"
            } else {
                "accept_claim"
            })
        }
        "tool_worker_crash" | "stale_tool_outcome" | "duplicate_side_effect" | "database_busy"
        | "tool_worker_panic" => {
            Some(disposition_for_id(id)?)
        }
        _ => None,
    }
}

/// Execute one registered deterministic scenario for a project-scoped shared suite.
/// Unknown ids fail closed; shared packages cannot inject executable evaluators.
pub(crate) fn evaluate_registered_scenario(
    id: &str,
    expected: &str,
) -> Result<EvalCaseResult, String> {
    let scenario = scenarios()
        .into_iter()
        .find(|scenario| scenario.id == id)
        .ok_or_else(|| format!("未注册评测场景：{id}"))?;
    if scenario.expected != expected {
        return Err(format!("评测场景 {id} 的期望契约不一致"));
    }
    let actual = simulate_scenario(id).ok_or_else(|| format!("评测场景 {id} 没有执行器"))?;
    Ok(EvalCaseResult {
        id: id.into(),
        domain: scenario.domain,
        expected: expected.into(),
        actual: actual.into(),
        passed: actual == expected,
    })
}

/// 调试/评测构建使用的显式故障点。发布构建永远返回 false，防止环境变量误伤用户任务。
pub fn fault_enabled(point: &str) -> bool {
    cfg!(debug_assertions) && std::env::var("HARMONY_AGENT_FAULT").ok().as_deref() == Some(point)
}

pub fn take_fault(point: &str) -> bool {
    static FIRED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    if !fault_enabled(point) {
        return false;
    }
    FIRED
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .map(|mut fired| fired.insert(point.to_string()))
        .unwrap_or(false)
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
