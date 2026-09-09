//! UI/headless 共用的外层 Agent executor 状态所有者。
//!
//! 本层仍保持纯状态、无 IO：adapter 负责 Provider、数据库、事件和工具执行；这里统一持有
//! 单轮路由、工具循环治理与唯一终止原因，避免三套跨轮状态在不同 adapter 中独立装配。

use crate::agent::acceptance::AcceptanceReport;
use crate::agent::agent_kernel::{
    decide_stop_candidate, KernelRunState, KernelRunTermination, KernelStopDecision,
};
use crate::agent::kernel_loop::{
    KernelBudgetVerdict, KernelLoopGovernor, KernelLoopVerdict, KernelRoundCounters,
    KernelRoundDecision, KernelRoundInput, KernelRoundRouter, KernelToolBudgetGate,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelRunPermit {
    Proceed { round: u64, remaining: Duration },
    Halt(KernelRunTermination),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelToolAttemptDecision {
    Observed {
        attempt: u64,
        verdict: KernelLoopVerdict,
    },
    Halt {
        reason: KernelRunTermination,
        attempted: u64,
        limit: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelExecutorFinalization {
    FixedRounds,
    Acceptance {
        governance_exhausted: bool,
        acceptance_passed: bool,
        completion_confirmed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct KernelExecutorLimits {
    pub round_limit: Option<u64>,
    pub tool_attempt_limit: Option<u64>,
    pub remediation_limit: usize,
}

impl Default for KernelExecutorLimits {
    fn default() -> Self {
        Self {
            round_limit: None,
            tool_attempt_limit: None,
            remediation_limit: usize::MAX,
        }
    }
}

/// 跨 adapter 稳定的 executor 最终快照，可直接写入桌面 run event 或 headless trajectory。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct KernelExecutorSnapshot {
    pub limits: KernelExecutorLimits,
    pub steps: u64,
    pub tool_attempts: u64,
    pub remediation_rounds: usize,
    pub loop_breaks: usize,
    pub round_counters: KernelRoundCounters,
    pub termination_reason: String,
    pub failure_taxonomy: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct KernelExecutorState {
    limits: KernelExecutorLimits,
    rounds: KernelRoundRouter,
    tools: KernelLoopGovernor,
    run: KernelRunState,
    completed_rounds: u64,
    tool_attempts: u64,
    remediation_rounds: usize,
}

impl KernelExecutorState {
    fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(limits: KernelExecutorLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    pub fn decide_round(&mut self, input: &KernelRoundInput<'_>) -> KernelRoundDecision {
        let decision = self.rounds.decide(input);
        if matches!(
            decision.control,
            crate::agent::kernel_loop::KernelRoundControl::StopEmpty { .. }
        ) {
            self.terminate(KernelRunTermination::EmptyRoundsExhausted);
        }
        decision
    }

    fn completed_rounds(&self) -> u64 {
        self.completed_rounds
    }

    /// 每次 Provider 请求前的原子安全点。已有终态与回合上限先于新一轮 deadline/取消，
    /// deadline 优先于取消；仅在放行时推进计数并返回 1-based round 与剩余墙钟预算。
    pub fn begin_round(
        &mut self,
        cancelled: bool,
        elapsed: Duration,
        deadline: Duration,
    ) -> KernelRunPermit {
        if let Some(reason) = self.termination() {
            return KernelRunPermit::Halt(reason);
        }
        if self
            .limits
            .round_limit
            .is_some_and(|limit| self.completed_rounds >= limit)
        {
            self.terminate(KernelRunTermination::MaxStepsExceeded);
            return KernelRunPermit::Halt(KernelRunTermination::MaxStepsExceeded);
        }
        if elapsed >= deadline {
            self.terminate(KernelRunTermination::DeadlineExceeded);
            return KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded);
        }
        if cancelled {
            self.terminate(KernelRunTermination::UserCancelled);
            return KernelRunPermit::Halt(KernelRunTermination::UserCancelled);
        }
        self.completed_rounds = self.completed_rounds.saturating_add(1);
        KernelRunPermit::Proceed {
            round: self.completed_rounds,
            remaining: deadline.saturating_sub(elapsed),
        }
    }

    /// 工具执行前的原子入口。先计入模型产生的尝试，再执行固定预算裁决与循环观察；
    /// `Some(limit)` 用于固定硬上限，`None` 用于 adapter 的动态预算门。
    /// 权限拒绝前也必须调用，使重复的非法尝试无法绕过循环治理。
    pub fn begin_tool_attempt(
        &mut self,
        tool: &str,
        args: &str,
    ) -> KernelToolAttemptDecision {
        let limit = self.limits.tool_attempt_limit;
        if let Some(reason) = self.termination() {
            return KernelToolAttemptDecision::Halt {
                reason,
                attempted: self.tool_attempts,
                limit,
            };
        }
        self.tool_attempts = self.tool_attempts.saturating_add(1);
        let attempted = self.tool_attempts;
        if limit.is_some_and(|value| attempted > value) {
            self.terminate(KernelRunTermination::ToolCallBudgetExceeded);
            KernelToolAttemptDecision::Halt {
                reason: KernelRunTermination::ToolCallBudgetExceeded,
                attempted,
                limit,
            }
        } else {
            let verdict = self.tools.observe(tool, args);
            if matches!(
                verdict,
                KernelLoopVerdict::Halt {
                    final_halt: true,
                    ..
                }
            ) {
                self.terminate(KernelRunTermination::ToolLoopExhausted);
            }
            KernelToolAttemptDecision::Observed { attempt: attempted, verdict }
        }
    }

    fn tool_attempts(&self) -> u64 {
        self.tool_attempts
    }

    /// 桌面端动态工具预算裁决。loop-break 状态直接取 executor 真源；最终 Halt 在
    /// 裁决处锁定为精确工具预算终止原因，adapter 只负责展示和收尾 IO。
    pub fn decide_dynamic_tool_budget(
        &mut self,
        limit: usize,
        used: usize,
        recent_successes: usize,
        extensions: usize,
    ) -> KernelBudgetVerdict {
        if self.termination().is_some() {
            return KernelBudgetVerdict::Halt;
        }
        let verdict = KernelToolBudgetGate::check(
            limit,
            used,
            recent_successes,
            self.loop_breaks(),
            extensions,
        );
        if verdict == KernelBudgetVerdict::Halt {
            self.terminate(KernelRunTermination::ToolCallBudgetExceeded);
        }
        verdict
    }

    /// 成本账本更新后的原子安全点。超限时锁定成本终止原因；若此前已经终止，
    /// 返回首因而不覆盖，供 adapter 立即停止后续路由和工具执行。
    pub fn observe_cost_budget(
        &mut self,
        exceeded: bool,
    ) -> Option<KernelRunTermination> {
        if let Some(reason) = self.termination() {
            return Some(reason);
        }
        if exceeded {
            self.terminate(KernelRunTermination::CostBudgetExceeded);
            return Some(KernelRunTermination::CostBudgetExceeded);
        }
        None
    }

    pub fn decide_stop(
        &mut self,
        report: AcceptanceReport,
    ) -> KernelStopDecision {
        decide_stop_candidate(
            report,
            &mut self.remediation_rounds,
            self.limits.remediation_limit,
        )
    }

    /// 对没有后置 ship/review 门的 adapter 执行终态停止裁决。
    /// Accepted/Exhausted 在产生决策的同一处锁定精确终止原因，Remediate 保持运行态。
    pub fn decide_terminal_stop(
        &mut self,
        report: AcceptanceReport,
    ) -> KernelStopDecision {
        let decision = self.decide_stop(report);
        match &decision {
            KernelStopDecision::Accepted(_) => {
                self.terminate(KernelRunTermination::ModelAccepted);
            }
            KernelStopDecision::Exhausted(_) => {
                self.terminate(KernelRunTermination::AcceptanceExhausted);
            }
            KernelStopDecision::Remediate { .. } => {}
        }
        decision
    }

    pub fn remediation_rounds(&self) -> usize {
        self.remediation_rounds
    }

    pub fn loop_breaks(&self) -> usize {
        self.tools.loop_breaks()
    }

    pub fn round_counters(&self) -> KernelRoundCounters {
        self.rounds.counters()
    }

    fn terminate(&mut self, reason: KernelRunTermination) {
        self.run.terminate(reason);
    }

    fn finish(&mut self, round_limit: u64) -> Option<KernelRunTermination> {
        self.run.finish(self.completed_rounds, round_limit)
    }

    fn termination(&self) -> Option<KernelRunTermination> {
        self.run.termination()
    }

    fn final_snapshot(&self) -> Result<KernelExecutorSnapshot, String> {
        let termination = self
            .termination()
            .ok_or_else(|| "executor 尚未终止，禁止生成最终快照".to_string())?;
        Ok(KernelExecutorSnapshot {
            limits: self.limits,
            steps: self.completed_rounds,
            tool_attempts: self.tool_attempts,
            remediation_rounds: self.remediation_rounds,
            loop_breaks: self.loop_breaks(),
            round_counters: self.round_counters(),
            termination_reason: termination.as_str().to_string(),
            failure_taxonomy: termination.failure_taxonomy().map(str::to_string),
        })
    }

    /// executor 的唯一公共最终化入口。固定回合模式完成自然耗尽归因；桌面验收模式
    /// 组合治理、证据与完成确认。两种模式都要求非空终止原因并保持首因。
    pub fn finalize(
        &mut self,
        mode: KernelExecutorFinalization,
    ) -> Result<KernelExecutorSnapshot, String> {
        match mode {
            KernelExecutorFinalization::FixedRounds => {
                let round_limit = self
                    .limits
                    .round_limit
                    .ok_or_else(|| "固定回合最终化缺少 round_limit 配置".to_string())?;
                self.finish(round_limit);
            }
            KernelExecutorFinalization::Acceptance {
                governance_exhausted,
                acceptance_passed,
                completion_confirmed,
            } => {
                let fallback = if governance_exhausted {
                    KernelRunTermination::GovernanceExhausted
                } else if !acceptance_passed {
                    KernelRunTermination::AcceptanceExhausted
                } else if completion_confirmed {
                    KernelRunTermination::ModelAccepted
                } else {
                    KernelRunTermination::CompletionReviewExhausted
                };
                self.terminate(fallback);
            }
        }
        self.final_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::kernel_loop::{KernelRoundControl, KERNEL_TOOL_CALL_LOOP_THRESHOLD};

    fn observe(executor: &mut KernelExecutorState, tool: &str, args: &str) -> KernelLoopVerdict {
        match executor.begin_tool_attempt(tool, args) {
            KernelToolAttemptDecision::Observed { verdict, .. } => verdict,
            KernelToolAttemptDecision::Halt { reason, .. } => {
                panic!("工具观察被意外终止：{}", reason.as_str())
            }
        }
    }

    #[test]
    fn executor_owns_round_and_tool_state_across_calls() {
        let mut executor = KernelExecutorState::new();
        let empty = KernelRoundInput {
            text: "",
            has_reasoning: false,
            truncated: false,
            interrupted: false,
            has_native_tool_calls: false,
        };
        assert!(matches!(
            executor.decide_round(&empty).control,
            KernelRoundControl::RetryEmpty { .. }
        ));
        assert!(matches!(
            executor.decide_round(&empty).control,
            KernelRoundControl::StopEmpty { .. }
        ));

        let mut tools = KernelExecutorState::new();
        for _ in 0..KERNEL_TOOL_CALL_LOOP_THRESHOLD - 1 {
            assert_eq!(
                observe(&mut tools, "read_file", r#"{"path":"a.rs"}"#),
                KernelLoopVerdict::Proceed
            );
        }
        assert!(matches!(
            observe(&mut tools, "read_file", r#"{"path":"a.rs"}"#),
            KernelLoopVerdict::Halt { .. }
        ));
        assert_eq!(tools.loop_breaks(), 1);
    }

    #[test]
    fn executor_preserves_specific_termination_when_finishing() {
        let mut executor = KernelExecutorState::new();
        assert!(matches!(
            executor.begin_round(false, Duration::ZERO, Duration::from_secs(10)),
            KernelRunPermit::Proceed { round: 1, .. }
        ));
        assert!(matches!(
            executor.begin_round(false, Duration::ZERO, Duration::from_secs(10)),
            KernelRunPermit::Proceed { round: 2, .. }
        ));
        assert_eq!(executor.completed_rounds(), 2);
        executor.terminate(KernelRunTermination::ToolCallBudgetExceeded);
        assert_eq!(
            executor.finish(2),
            Some(KernelRunTermination::ToolCallBudgetExceeded)
        );
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::ToolCallBudgetExceeded)
        );
    }

    #[test]
    fn executor_run_permit_prioritizes_deadline_and_reports_remaining_time() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(
            executor.begin_round(
                false,
                Duration::from_secs(4),
                Duration::from_secs(10),
            ),
            KernelRunPermit::Proceed {
                round: 1,
                remaining: Duration::from_secs(6)
            }
        );
        assert_eq!(
            executor.begin_round(
                true,
                Duration::from_secs(10),
                Duration::from_secs(10),
            ),
            KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded)
        );
        assert_eq!(executor.completed_rounds(), 1);
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::DeadlineExceeded)
        );
    }

    #[test]
    fn executor_run_permit_records_user_cancellation() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(
            executor.begin_round(
                true,
                Duration::from_secs(1),
                Duration::from_secs(10),
            ),
            KernelRunPermit::Halt(KernelRunTermination::UserCancelled)
        );
        assert_eq!(executor.completed_rounds(), 0);
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::UserCancelled)
        );
    }

    #[test]
    fn executor_tool_attempt_budget_counts_rejected_attempt() {
        let mut executor = KernelExecutorState::with_limits(KernelExecutorLimits {
            round_limit: Some(10),
            tool_attempt_limit: Some(2),
            remediation_limit: usize::MAX,
        });
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"1.rs"}"#),
            KernelToolAttemptDecision::Observed {
                attempt: 1,
                verdict: KernelLoopVerdict::Proceed,
            }
        );
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"2.rs"}"#),
            KernelToolAttemptDecision::Observed {
                attempt: 2,
                verdict: KernelLoopVerdict::Proceed,
            }
        );
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"3.rs"}"#),
            KernelToolAttemptDecision::Halt {
                reason: KernelRunTermination::ToolCallBudgetExceeded,
                attempted: 3,
                limit: Some(2)
            }
        );
        assert_eq!(executor.tool_attempts(), 3);
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::ToolCallBudgetExceeded)
        );
        let snapshot = executor
            .finalize(KernelExecutorFinalization::FixedRounds)
            .expect("已终止 executor 应生成最终快照");
        assert_eq!(snapshot.tool_attempts, 3);
        assert_eq!(snapshot.termination_reason, "max_tool_calls_exceeded");
        assert_eq!(
            snapshot.failure_taxonomy.as_deref(),
            Some("max_tool_calls_exceeded")
        );
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["limits"]["round_limit"], 10);
        assert_eq!(json["limits"]["tool_attempt_limit"], 2);
        assert_eq!(json["round_counters"]["empty_rounds"], 0);
        assert_eq!(json["loop_breaks"], 0);
    }

    #[test]
    fn executor_rejects_final_snapshot_without_termination() {
        let mut executor = KernelExecutorState::with_limits(KernelExecutorLimits {
            round_limit: Some(2),
            ..KernelExecutorLimits::default()
        });
        assert!(matches!(
            executor.begin_round(false, Duration::ZERO, Duration::from_secs(10)),
            KernelRunPermit::Proceed { .. }
        ));
        let error = executor
            .finalize(KernelExecutorFinalization::FixedRounds)
            .expect_err("未终止且未跑满时必须失败关闭");
        assert!(error.contains("尚未终止"));
    }

    #[test]
    fn fixed_round_finalization_requires_frozen_limit() {
        let mut executor = KernelExecutorState::new();
        let error = executor
            .finalize(KernelExecutorFinalization::FixedRounds)
            .expect_err("固定回合模式缺少创建期限制时必须失败关闭");
        assert!(error.contains("缺少 round_limit"));
    }

    #[test]
    fn executor_owns_bounded_stop_remediation_count() {
        let mut executor = KernelExecutorState::with_limits(KernelExecutorLimits {
            remediation_limit: 1,
            ..KernelExecutorLimits::default()
        });
        let report = crate::agent::acceptance::evaluate_contract(
            &crate::agent::acceptance::GoalContract::compile("修改 a.rs"),
            &[],
        );
        assert!(matches!(
            executor.decide_stop(report.clone()),
            KernelStopDecision::Remediate { round: 1, .. }
        ));
        assert!(matches!(
            executor.decide_stop(report),
            KernelStopDecision::Exhausted(_)
        ));
        assert_eq!(executor.remediation_rounds(), 1);
    }

    #[test]
    fn executor_terminal_stop_maps_acceptance_outcomes_atomically() {
        let passed = crate::agent::acceptance::evaluate_contract(
            &crate::agent::acceptance::GoalContract::compile("解释代码"),
            &[],
        );
        let mut accepted = KernelExecutorState::with_limits(KernelExecutorLimits {
            remediation_limit: 1,
            ..KernelExecutorLimits::default()
        });
        assert!(matches!(
            accepted.decide_terminal_stop(passed),
            KernelStopDecision::Accepted(_)
        ));
        assert_eq!(
            accepted.termination(),
            Some(KernelRunTermination::ModelAccepted)
        );

        let missing = crate::agent::acceptance::evaluate_contract(
            &crate::agent::acceptance::GoalContract::compile("修改 a.rs"),
            &[],
        );
        let mut exhausted = KernelExecutorState::with_limits(KernelExecutorLimits {
            remediation_limit: 0,
            ..KernelExecutorLimits::default()
        });
        assert!(matches!(
            exhausted.decide_terminal_stop(missing),
            KernelStopDecision::Exhausted(_)
        ));
        assert_eq!(
            exhausted.termination(),
            Some(KernelRunTermination::AcceptanceExhausted)
        );
    }

    #[test]
    fn executor_maps_terminal_policy_decisions_to_run_reason() {
        let mut rounds = KernelExecutorState::new();
        let empty = KernelRoundInput {
            text: "",
            has_reasoning: false,
            truncated: false,
            interrupted: false,
            has_native_tool_calls: false,
        };
        rounds.decide_round(&empty);
        rounds.decide_round(&empty);
        assert_eq!(
            rounds.termination(),
            Some(KernelRunTermination::EmptyRoundsExhausted)
        );

        let mut tools = KernelExecutorState::new();
        for cycle in 0..=crate::agent::kernel_loop::KERNEL_MAX_LOOP_BREAKS {
            let path = format!(r#"{{"path":"{cycle}.rs"}}"#);
            for _ in 0..KERNEL_TOOL_CALL_LOOP_THRESHOLD {
                observe(&mut tools, "read_file", &path);
            }
            if cycle < crate::agent::kernel_loop::KERNEL_MAX_LOOP_BREAKS {
                observe(&mut tools, "write_file", r#"{"path":"reset.rs"}"#);
            }
        }
        assert_eq!(
            tools.termination(),
            Some(KernelRunTermination::ToolLoopExhausted)
        );
    }

    #[test]
    fn executor_terminal_state_is_absorbing_at_provider_boundary() {
        let mut executor = KernelExecutorState::new();
        executor.terminate(KernelRunTermination::ToolLoopExhausted);

        assert_eq!(
            executor.begin_round(
                true,
                Duration::from_secs(20),
                Duration::from_secs(10),
            ),
            KernelRunPermit::Halt(KernelRunTermination::ToolLoopExhausted)
        );
        assert_eq!(executor.completed_rounds(), 0);
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::ToolLoopExhausted)
        );
    }

    #[test]
    fn executor_terminal_state_is_absorbing_at_tool_boundary() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"a.rs"}"#),
            KernelToolAttemptDecision::Observed {
                attempt: 1,
                verdict: KernelLoopVerdict::Proceed,
            }
        );
        executor.terminate(KernelRunTermination::UserCancelled);
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"b.rs"}"#),
            KernelToolAttemptDecision::Halt {
                reason: KernelRunTermination::UserCancelled,
                attempted: 1,
                limit: None,
            }
        );
        assert_eq!(executor.tool_attempts(), 1);
    }

    #[test]
    fn executor_owns_dynamic_tool_budget_termination() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(
            executor.decide_dynamic_tool_budget(100, 50, 0, 0),
            KernelBudgetVerdict::Proceed
        );
        assert!(matches!(
            executor.decide_dynamic_tool_budget(100, 100, 5, 0),
            KernelBudgetVerdict::Extend { new_limit } if new_limit > 100
        ));
        assert_eq!(
            executor.decide_dynamic_tool_budget(100, 100, 2, 0),
            KernelBudgetVerdict::Halt
        );
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::ToolCallBudgetExceeded)
        );
    }

    #[test]
    fn executor_does_not_mark_unconfirmed_completion_as_accepted() {
        let mut unconfirmed = KernelExecutorState::new();
        let snapshot = unconfirmed
            .finalize(KernelExecutorFinalization::Acceptance {
                governance_exhausted: false,
                acceptance_passed: true,
                completion_confirmed: false,
            })
            .unwrap();
        assert_eq!(snapshot.termination_reason, "completion_review_exhausted");
        assert_eq!(
            snapshot.failure_taxonomy.as_deref(),
            Some("completion_review_exhausted")
        );

        let mut accepted = KernelExecutorState::new();
        assert_eq!(
            accepted
                .finalize(KernelExecutorFinalization::Acceptance {
                    governance_exhausted: false,
                    acceptance_passed: true,
                    completion_confirmed: true,
                })
                .unwrap()
                .termination_reason,
            "model_accepted"
        );
    }

    #[test]
    fn executor_enforces_round_limit_before_starting_next_provider_call() {
        let mut executor = KernelExecutorState::with_limits(KernelExecutorLimits {
            round_limit: Some(1),
            ..KernelExecutorLimits::default()
        });
        assert!(matches!(
            executor.begin_round(false, Duration::ZERO, Duration::from_secs(10)),
            KernelRunPermit::Proceed { round: 1, .. }
        ));
        assert_eq!(
            executor.begin_round(true, Duration::from_secs(20), Duration::from_secs(10)),
            KernelRunPermit::Halt(KernelRunTermination::MaxStepsExceeded)
        );
        assert_eq!(executor.completed_rounds(), 1);
    }

    #[test]
    fn executor_owns_cost_budget_termination_and_preserves_first_reason() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(executor.observe_cost_budget(false), None);
        assert_eq!(
            executor.observe_cost_budget(true),
            Some(KernelRunTermination::CostBudgetExceeded)
        );

        let mut already_stopped = KernelExecutorState::new();
        already_stopped.terminate(KernelRunTermination::UserCancelled);
        assert_eq!(
            already_stopped.observe_cost_budget(true),
            Some(KernelRunTermination::UserCancelled)
        );
    }
}
