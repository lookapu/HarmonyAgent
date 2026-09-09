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

/// 跨 adapter 稳定的 executor 最终快照，可直接写入桌面 run event 或 headless trajectory。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct KernelExecutorSnapshot {
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
    rounds: KernelRoundRouter,
    tools: KernelLoopGovernor,
    run: KernelRunState,
    completed_rounds: u64,
    tool_attempts: u64,
    remediation_rounds: usize,
}

impl KernelExecutorState {
    pub fn new() -> Self {
        Self::default()
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

    pub fn completed_rounds(&self) -> u64 {
        self.completed_rounds
    }

    /// 每次 Provider 请求前的原子安全点。deadline 优先于取消，与桌面历史顺序一致；
    /// 仅在放行时推进一次回合计数，同时返回 1-based round 与剩余墙钟预算。
    pub fn begin_round(
        &mut self,
        cancelled: bool,
        elapsed: Duration,
        deadline: Duration,
    ) -> KernelRunPermit {
        if let Some(reason) = self.termination() {
            return KernelRunPermit::Halt(reason);
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
        limit: Option<u64>,
    ) -> KernelToolAttemptDecision {
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

    pub fn tool_attempts(&self) -> u64 {
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

    pub fn decide_stop(
        &mut self,
        report: AcceptanceReport,
        max_remediation_rounds: usize,
    ) -> KernelStopDecision {
        decide_stop_candidate(report, &mut self.remediation_rounds, max_remediation_rounds)
    }

    /// 对没有后置 ship/review 门的 adapter 执行终态停止裁决。
    /// Accepted/Exhausted 在产生决策的同一处锁定精确终止原因，Remediate 保持运行态。
    pub fn decide_terminal_stop(
        &mut self,
        report: AcceptanceReport,
        max_remediation_rounds: usize,
    ) -> KernelStopDecision {
        let decision = self.decide_stop(report, max_remediation_rounds);
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

    pub fn terminate(&mut self, reason: KernelRunTermination) {
        self.run.terminate(reason);
    }

    pub fn finish(&mut self, round_limit: u64) -> Option<KernelRunTermination> {
        self.run.finish(self.completed_rounds, round_limit)
    }

    pub fn termination(&self) -> Option<KernelRunTermination> {
        self.run.termination()
    }

    fn final_snapshot(&self) -> Result<KernelExecutorSnapshot, String> {
        let termination = self
            .termination()
            .ok_or_else(|| "executor 尚未终止，禁止生成最终快照".to_string())?;
        Ok(KernelExecutorSnapshot {
            steps: self.completed_rounds,
            tool_attempts: self.tool_attempts,
            remediation_rounds: self.remediation_rounds,
            loop_breaks: self.loop_breaks(),
            round_counters: self.round_counters(),
            termination_reason: termination.as_str().to_string(),
            failure_taxonomy: termination.failure_taxonomy().map(str::to_string),
        })
    }

    /// 有固定 round limit 的 executor：先完成自然耗尽归因，再生成非空终止快照。
    pub fn finish_and_snapshot(
        &mut self,
        round_limit: u64,
    ) -> Result<KernelExecutorSnapshot, String> {
        self.finish(round_limit);
        self.final_snapshot()
    }

    /// 由 adapter 提供无固定 round limit 时的最终回退原因；已存在的首个原因不会被覆盖。
    pub fn terminate_and_snapshot(
        &mut self,
        fallback: KernelRunTermination,
    ) -> KernelExecutorSnapshot {
        self.terminate(fallback);
        self.final_snapshot()
            .expect("terminate 后必须能够生成 executor 最终快照")
    }

    /// 桌面二阶段验收的最终归因：证据通过但完成复核未确认，不能记为 model accepted。
    /// 已存在的更具体终止原因仍由 `KernelRunState` 首因规则保留。
    pub fn finalize_acceptance_snapshot(
        &mut self,
        governance_exhausted: bool,
        acceptance_passed: bool,
        completion_confirmed: bool,
    ) -> KernelExecutorSnapshot {
        let fallback = if governance_exhausted {
            KernelRunTermination::GovernanceExhausted
        } else if !acceptance_passed {
            KernelRunTermination::AcceptanceExhausted
        } else if completion_confirmed {
            KernelRunTermination::ModelAccepted
        } else {
            KernelRunTermination::CompletionReviewExhausted
        };
        self.terminate_and_snapshot(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::kernel_loop::{KernelRoundControl, KERNEL_TOOL_CALL_LOOP_THRESHOLD};

    fn observe(executor: &mut KernelExecutorState, tool: &str, args: &str) -> KernelLoopVerdict {
        match executor.begin_tool_attempt(tool, args, None) {
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
            executor.begin_round(false, Duration::from_secs(4), Duration::from_secs(10)),
            KernelRunPermit::Proceed {
                round: 1,
                remaining: Duration::from_secs(6)
            }
        );
        assert_eq!(
            executor.begin_round(true, Duration::from_secs(10), Duration::from_secs(10)),
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
            executor.begin_round(true, Duration::from_secs(1), Duration::from_secs(10)),
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
        let mut executor = KernelExecutorState::new();
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"1.rs"}"#, Some(2)),
            KernelToolAttemptDecision::Observed {
                attempt: 1,
                verdict: KernelLoopVerdict::Proceed,
            }
        );
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"2.rs"}"#, Some(2)),
            KernelToolAttemptDecision::Observed {
                attempt: 2,
                verdict: KernelLoopVerdict::Proceed,
            }
        );
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"3.rs"}"#, Some(2)),
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
            .finish_and_snapshot(10)
            .expect("已终止 executor 应生成最终快照");
        assert_eq!(snapshot.tool_attempts, 3);
        assert_eq!(snapshot.termination_reason, "max_tool_calls_exceeded");
        assert_eq!(
            snapshot.failure_taxonomy.as_deref(),
            Some("max_tool_calls_exceeded")
        );
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["round_counters"]["empty_rounds"], 0);
        assert_eq!(json["loop_breaks"], 0);
    }

    #[test]
    fn executor_rejects_final_snapshot_without_termination() {
        let mut executor = KernelExecutorState::new();
        assert!(matches!(
            executor.begin_round(false, Duration::ZERO, Duration::from_secs(10)),
            KernelRunPermit::Proceed { .. }
        ));
        let error = executor
            .finish_and_snapshot(2)
            .expect_err("未终止且未跑满时必须失败关闭");
        assert!(error.contains("尚未终止"));
    }

    #[test]
    fn executor_owns_bounded_stop_remediation_count() {
        let mut executor = KernelExecutorState::new();
        let report = crate::agent::acceptance::evaluate_contract(
            &crate::agent::acceptance::GoalContract::compile("修改 a.rs"),
            &[],
        );
        assert!(matches!(
            executor.decide_stop(report.clone(), 1),
            KernelStopDecision::Remediate { round: 1, .. }
        ));
        assert!(matches!(
            executor.decide_stop(report, 1),
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
        let mut accepted = KernelExecutorState::new();
        assert!(matches!(
            accepted.decide_terminal_stop(passed, 1),
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
        let mut exhausted = KernelExecutorState::new();
        assert!(matches!(
            exhausted.decide_terminal_stop(missing, 0),
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
            executor.begin_round(true, Duration::from_secs(20), Duration::from_secs(10)),
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
            executor.begin_tool_attempt("read_file", r#"{"path":"a.rs"}"#, None),
            KernelToolAttemptDecision::Observed {
                attempt: 1,
                verdict: KernelLoopVerdict::Proceed,
            }
        );
        executor.terminate(KernelRunTermination::UserCancelled);
        assert_eq!(
            executor.begin_tool_attempt("read_file", r#"{"path":"b.rs"}"#, None),
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
        let snapshot = unconfirmed.finalize_acceptance_snapshot(false, true, false);
        assert_eq!(snapshot.termination_reason, "completion_review_exhausted");
        assert_eq!(
            snapshot.failure_taxonomy.as_deref(),
            Some("completion_review_exhausted")
        );

        let mut accepted = KernelExecutorState::new();
        assert_eq!(
            accepted
                .finalize_acceptance_snapshot(false, true, true)
                .termination_reason,
            "model_accepted"
        );
    }
}
