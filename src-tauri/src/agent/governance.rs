//! 长任务动态预算、租约和质量指标的纯策略层。

use serde::{Deserialize, Serialize};

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureSignal {
    StreamBeforeDelta, StreamAfterDelta, ModelTruncated, ReadTimeout, WriteTimeout,
    RestartPreparedEffect, ApprovalTimeout, StaleTerminal, MissingEvidence,
    BudgetExhausted, SubagentMissingEvidence,
}

#[cfg(test)]
pub fn reliability_disposition(signal: FailureSignal) -> &'static str {
    match signal {
        FailureSignal::StreamBeforeDelta => "replay_same_request",
        FailureSignal::StreamAfterDelta => "continue_from_checkpoint",
        FailureSignal::ModelTruncated => "bounded_continuation",
        FailureSignal::ReadTimeout => "safe_retry",
        FailureSignal::WriteTimeout => "verify_before_replay",
        FailureSignal::RestartPreparedEffect => "verify_effects",
        FailureSignal::ApprovalTimeout => "fail_closed",
        FailureSignal::StaleTerminal => "terminal_state_immutable",
        FailureSignal::MissingEvidence => "automatic_remediation",
        FailureSignal::BudgetExhausted => "unfinished_with_checkpoint",
        FailureSignal::SubagentMissingEvidence => "reject_claim",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionBudget {
    pub complexity: u8,
    pub tool_rounds: usize,
    pub duration_ms: i64,
    pub remediation_rounds: usize,
    pub lease_ms: i64,
    pub allow_model_fallback: bool,
}

impl ExecutionBudget {
    pub fn for_contract(contract: &crate::agent::acceptance::GoalContract, recovery_attempt: usize) -> Self {
        let criteria = contract.criteria.len();
        let broad = ["全面", "完整", "商业级", "重构", "架构", "全链路"]
            .iter().filter(|word| contract.original_goal.contains(**word)).count();
        let complexity = (1 + criteria + broad * 2 + usize::from(recovery_attempt > 0)).min(10) as u8;
        Self {
            complexity,
            tool_rounds: 24usize.saturating_add(complexity as usize * 12),
            duration_ms: 10 * 60_000 + complexity as i64 * 5 * 60_000,
            remediation_rounds: (2 + complexity as usize / 3).clamp(2, 5),
            lease_ms: 90_000 + complexity as i64 * 15_000,
            allow_model_fallback: complexity >= 4,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunQualitySnapshot {
    pub schema_version: u32,
    pub acceptance_passed: bool,
    pub evidence_criteria: usize,
    pub remediation_count: usize,
    pub recovered: bool,
    pub exhausted: bool,
    pub score: u8,
}

impl RunQualitySnapshot {
    pub fn calculate(
        acceptance: &crate::agent::acceptance::AcceptanceReport,
        remediation_count: usize,
        recovered: bool,
        exhausted: bool,
    ) -> Self {
        let mut score: i16 = 100;
        if !acceptance.passed { score -= 45; }
        if exhausted { score -= 25; }
        score -= (remediation_count.min(5) * 4) as i16;
        if recovered && acceptance.passed { score += 3; }
        Self {
            schema_version: 1,
            acceptance_passed: acceptance.passed,
            evidence_criteria: acceptance.criteria.iter().filter(|criterion| criterion.passed).count(),
            remediation_count,
            recovered,
            exhausted,
            score: score.clamp(0, 100) as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complex_contract_receives_more_budget() {
        let simple = ExecutionBudget::for_contract(&crate::agent::acceptance::GoalContract::compile("解释代码"), 0);
        let complex = ExecutionBudget::for_contract(&crate::agent::acceptance::GoalContract::compile("全面重构、测试、构建、提交并推送"), 1);
        assert!(complex.tool_rounds > simple.tool_rounds);
        assert!(complex.duration_ms > simple.duration_ms);
        assert!(complex.allow_model_fallback);
    }

    #[test]
    fn reliability_fixture_covers_commercial_failure_domains() {
        let scenarios: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/agent_reliability_scenarios.json"
        )).unwrap();
        let rows = scenarios.as_array().unwrap();
        let signals = [
            FailureSignal::StreamBeforeDelta, FailureSignal::StreamAfterDelta,
            FailureSignal::ModelTruncated, FailureSignal::ReadTimeout,
            FailureSignal::WriteTimeout, FailureSignal::RestartPreparedEffect,
            FailureSignal::ApprovalTimeout, FailureSignal::StaleTerminal,
            FailureSignal::MissingEvidence, FailureSignal::BudgetExhausted,
            FailureSignal::SubagentMissingEvidence,
        ];
        assert_eq!(rows.len(), signals.len());
        for (row, signal) in rows.iter().zip(signals) {
            assert_eq!(row["expected"], reliability_disposition(signal));
        }
        let domains = rows.iter().filter_map(|row| row["domain"].as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(domains.len() >= 8, "fault domains={domains:?}");
    }
}
