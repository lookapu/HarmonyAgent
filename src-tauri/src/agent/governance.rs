//! 长任务动态预算、租约和质量指标的纯策略层。

use serde::{Deserialize, Serialize};

pub const MAX_TOOL_BUDGET_EXTENSIONS: usize = 2;
pub const MAX_EFFECTIVE_TOOL_ROUNDS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureSignal {
    StreamBeforeDelta, StreamAfterDelta, ModelTruncated, ReadTimeout, WriteTimeout,
    RestartPreparedEffect, ApprovalTimeout, StaleTerminal, MissingEvidence,
    BudgetExhausted, SubagentMissingEvidence,
    ToolWorkerCrash, StaleToolOutcome, DuplicateSideEffect, DatabaseBusy, ToolWorkerPanic,
}

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
        FailureSignal::ToolWorkerCrash => "effect_aware_recovery",
        FailureSignal::StaleToolOutcome => "owner_fenced",
        FailureSignal::DuplicateSideEffect => "idempotency_blocked",
        FailureSignal::DatabaseBusy => "bounded_backpressure",
        FailureSignal::ToolWorkerPanic => "isolate_call_and_continue",
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

/// 只有近期持续产生成功工具证据、且未触发循环检测时才允许扩容；最多两次，
/// 防止复杂任务被固定上限误杀，也不让打转任务无限消耗。
pub fn extend_tool_budget(
    current: usize,
    recent_successes: usize,
    loop_breaks: usize,
    extensions: usize,
) -> Option<usize> {
    if recent_successes < 3 || loop_breaks > 0 || extensions >= MAX_TOOL_BUDGET_EXTENSIONS { return None; }
    Some(
        current
            .saturating_add((current / 3).max(12))
            .min(MAX_EFFECTIVE_TOOL_ROUNDS),
    )
}

/// 契约一条可验证判据都没有时，本次运行的质量分恒为此值。
///
/// `GoalContract::compile` 只在目标里认得变更/构建/测试/部署/提交/推送时才生成判据，
/// 所以「现在项目什么情况？」这类**纯问答**必然落进 `criteria == []`。此时
/// `AcceptanceReport::passed` 只是 `blockers.is_empty()` 作用在**空集合**上的结果，
/// 恒为 `true`——运行内核其实**没有对完成声明做过任何裁决**。
///
/// 过去这里报 100 分，于是 `average_quality_score` 把「从没被检查过」
/// 显示成「完美通过」（Cost 页 `scoreTone(>=90)` 还会转成绿色）。本机实测：
/// 一轮 9 字提问烧掉 21 万输入 token、25 次工具调用全是
/// `contribution_state=no_direct_acceptance_evidence`，照样满分。
///
/// 取 0 而不是某个中间分，是为了让**忘记看 `unscored` 标志的下游显示为不可信**，
/// 而不是显示为完美——错向安全那一侧。真正该改的下游见 `commands/reliability.rs`：
/// 未受裁决的运行必须**排除在均值之外**，而不是折算成 0 拉低整体。
pub const UNSCORED_QUALITY_SCORE: u8 = 0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunQualitySnapshot {
    pub schema_version: u32,
    pub acceptance_passed: bool,
    /// 契约声明的判据总数。`0` 表示本次运行从未受证据裁决。
    pub criteria_declared: usize,
    /// 判据通过数。`criteria_declared == 0` 时它必然是 0，**不能**读成「判据全挂」。
    pub evidence_criteria: usize,
    pub remediation_count: usize,
    pub recovered: bool,
    pub exhausted: bool,
    /// `unscored == true` 时本字段**无意义**，恒为 [`UNSCORED_QUALITY_SCORE`]；
    /// 用它之前先看 `unscored`。
    pub score: u8,
    /// 契约没有可验证判据 ⇒ 完成声明**未被裁决**，不可当作通过。
    ///
    /// 单独一个标志位而不是让消费者去猜 `evidence_criteria == 0`：真实失败的运行
    /// 同样可能是 0 条通过（判据全挂时 `score` 是 55 而非 0），两者只有靠这个标志区分。
    pub unscored: bool,
}

impl RunQualitySnapshot {
    pub fn calculate(
        acceptance: &crate::agent::acceptance::AcceptanceReport,
        remediation_count: usize,
        recovered: bool,
        exhausted: bool,
    ) -> Self {
        let criteria_declared = acceptance.criteria.len();
        let unscored = criteria_declared == 0;
        let evidence_criteria =
            acceptance.criteria.iter().filter(|criterion| criterion.passed).count();
        let mut score: i16 = 100;
        if !acceptance.passed { score -= 45; }
        if exhausted { score -= 25; }
        score -= (remediation_count.min(5) * 4) as i16;
        if recovered && acceptance.passed { score += 3; }
        // 未受裁决时，上面每一项加减都建立在「判过了」这个前提上，而前提不成立，
        // 整段分数无意义——不折算、不取中间值，直接归零。
        let score = if unscored { UNSCORED_QUALITY_SCORE as i16 } else { score.clamp(0, 100) };
        Self {
            schema_version: 2,
            acceptance_passed: acceptance.passed,
            criteria_declared,
            evidence_criteria,
            remediation_count,
            recovered,
            exhausted,
            score: score as u8,
            unscored,
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
    fn budget_only_extends_for_real_progress() {
        assert_eq!(extend_tool_budget(60, 5, 0, 0), Some(80));
        assert_eq!(extend_tool_budget(60, 1, 0, 0), None);
        assert_eq!(extend_tool_budget(60, 5, 1, 0), None);
        assert_eq!(extend_tool_budget(60, 5, 0, 2), None);
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
            FailureSignal::ToolWorkerCrash, FailureSignal::StaleToolOutcome,
            FailureSignal::DuplicateSideEffect, FailureSignal::DatabaseBusy,
            FailureSignal::ToolWorkerPanic,
        ];
        assert_eq!(rows.len(), signals.len());
        for (row, signal) in rows.iter().zip(signals) {
            assert_eq!(row["expected"], reliability_disposition(signal));
        }
        let domains = rows.iter().filter_map(|row| row["domain"].as_str())
            .collect::<std::collections::HashSet<_>>();
        assert!(domains.len() >= 8, "fault domains={domains:?}");
    }

    /// 反例取自真实会话 047424ea：一句「现在项目什么情况？」跑出
    /// `score=100 / evidence_criteria=0 / acceptance_passed=true`。
    /// `passed=true` 是空集合上的 `blockers.is_empty()`，不代表被裁决过。
    #[test]
    fn pure_qa_run_is_unscored_not_perfect() {
        use crate::agent::acceptance::{evaluate, GoalContract};
        // 契约里确实一条判据都没有（目标不含变更/构建/测试/部署/提交/推送）
        let contract = GoalContract::compile("现在项目什么情况？");
        assert!(contract.criteria.is_empty(), "本反例依赖「判据为空」这一前提");

        // 一次成功的工具调用也不能把「未受裁决」变成「受裁决」
        let ev = crate::agent::acceptance::ToolEvidence {
            tool: "deep_scan", args: "{}", output: "361 files", succeeded: true,
        };
        let report = evaluate("现在项目什么情况？", &[ev]);
        assert!(report.passed, "空判据时 passed 本就恒真——这正是问题所在");
        assert!(report.criteria.is_empty());
        assert_eq!(report.evidence_count, 0);

        let snap = RunQualitySnapshot::calculate(&report, 0, false, false);
        assert!(snap.unscored, "判据为空必须标为未受裁决");
        assert_eq!(snap.criteria_declared, 0);
        assert_eq!(snap.evidence_criteria, 0);
        assert_eq!(snap.score, UNSCORED_QUALITY_SCORE);
        assert_ne!(snap.score, 100, "0 判据绝不能报满分");
        // 兜底：即便 remediation / recovered / exhausted 全部拉满也不许涨分
        let loaded = RunQualitySnapshot::calculate(&report, 5, true, true);
        assert_eq!(loaded.score, UNSCORED_QUALITY_SCORE, "未受裁决的运行不参与打分");
    }

    /// 判据非空时打分行为必须**原样保留**，否则这是在拿纯问答修真实回归。
    #[test]
    fn scored_runs_keep_their_score() {
        use crate::agent::acceptance::evaluate;
        fn ev(tool: &str) -> crate::agent::acceptance::ToolEvidence<'_> {
            crate::agent::acceptance::ToolEvidence {
                tool, args: "{}", output: "ok", succeeded: true,
            }
        }
        // 「修复并构建」→ 3 条判据，全部有证据
        let good = evaluate("修复并构建", &[ev("edit_file"), ev("build_project")]);
        assert!(good.passed, "blockers={:?}", good.blockers);
        let snap = RunQualitySnapshot::calculate(&good, 0, false, false);
        assert!(!snap.unscored);
        assert_eq!(snap.criteria_declared, 3);
        assert!(snap.evidence_criteria >= 3, "{}", snap.evidence_criteria);
        assert_eq!(snap.score, 100, "受裁决且全通过仍是 100");

        // 判据有但没证据 → 必须真扣分，不能被 unscored 分支吞掉
        let bad = evaluate("修复并构建", &[]);
        assert!(!bad.passed);
        let snap = RunQualitySnapshot::calculate(&bad, 0, false, false);
        assert!(!snap.unscored, "判据非空就不是未受裁决");
        assert_eq!(snap.criteria_declared, 3);
        assert_eq!(snap.evidence_criteria, 0);
        assert_eq!(snap.score, 55, "100 - 45（验收未通过）");
    }
}
