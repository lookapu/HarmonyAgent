//! UI/headless 共用的外层 Agent executor 状态所有者。
//!
//! 本层仍保持纯状态、无 IO：adapter 负责 Provider、数据库、事件和工具执行；这里统一持有
//! 单轮路由、工具循环治理与唯一终止原因，避免三套跨轮状态在不同 adapter 中独立装配。

use crate::agent::agent_kernel::{KernelRunState, KernelRunTermination};
use crate::agent::kernel_loop::{
    KernelLoopGovernor, KernelLoopVerdict, KernelRoundDecision, KernelRoundInput,
    KernelRoundRouter,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelRunPermit {
    Proceed { remaining: Duration },
    Halt(KernelRunTermination),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelToolAttemptPermit {
    Proceed { attempt: u64 },
    Halt { attempted: u64, limit: u64 },
}

#[derive(Clone, Debug, Default)]
pub struct KernelExecutorState {
    rounds: KernelRoundRouter,
    tools: KernelLoopGovernor,
    run: KernelRunState,
    completed_rounds: u64,
    tool_attempts: u64,
}

impl KernelExecutorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decide_round(&mut self, input: &KernelRoundInput<'_>) -> KernelRoundDecision {
        self.rounds.decide(input)
    }

    /// Provider 请求真正开始前推进一次回合计数，并返回 1-based round number。
    pub fn start_round(&mut self) -> u64 {
        self.completed_rounds = self.completed_rounds.saturating_add(1);
        self.completed_rounds
    }

    pub fn completed_rounds(&self) -> u64 {
        self.completed_rounds
    }

    /// 每次 Provider 请求前的共用安全点。deadline 优先于取消，与桌面历史顺序一致。
    pub fn permit_run(
        &mut self,
        cancelled: bool,
        elapsed: Duration,
        deadline: Duration,
    ) -> KernelRunPermit {
        if elapsed >= deadline {
            self.terminate(KernelRunTermination::DeadlineExceeded);
            return KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded);
        }
        if cancelled {
            self.terminate(KernelRunTermination::UserCancelled);
            return KernelRunPermit::Halt(KernelRunTermination::UserCancelled);
        }
        KernelRunPermit::Proceed {
            remaining: deadline.saturating_sub(elapsed),
        }
    }

    pub fn observe_tool(&mut self, tool: &str, args: &str) -> KernelLoopVerdict {
        self.tools.observe(tool, args)
    }

    /// 对模型产生的每一个工具调用尝试计数（包括随后被策略拒绝的调用）。
    pub fn permit_tool_attempt(&mut self, limit: u64) -> KernelToolAttemptPermit {
        let attempted = self.record_tool_attempt();
        if attempted > limit {
            self.terminate(KernelRunTermination::ToolCallBudgetExceeded);
            KernelToolAttemptPermit::Halt {
                attempted,
                limit,
            }
        } else {
            KernelToolAttemptPermit::Proceed { attempt: attempted }
        }
    }

    /// 仅记账，不施加固定上限；用于拥有动态/可扩展工具预算的 adapter。
    pub fn record_tool_attempt(&mut self) -> u64 {
        self.tool_attempts = self.tool_attempts.saturating_add(1);
        self.tool_attempts
    }

    pub fn tool_attempts(&self) -> u64 {
        self.tool_attempts
    }

    pub fn loop_breaks(&self) -> usize {
        self.tools.loop_breaks()
    }

    pub fn round_counters(&self) -> (usize, usize, usize, usize, usize) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::kernel_loop::{KernelRoundControl, KERNEL_TOOL_CALL_LOOP_THRESHOLD};

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

        for _ in 0..KERNEL_TOOL_CALL_LOOP_THRESHOLD - 1 {
            assert_eq!(
                executor.observe_tool("read_file", r#"{"path":"a.rs"}"#),
                KernelLoopVerdict::Proceed
            );
        }
        assert!(matches!(
            executor.observe_tool("read_file", r#"{"path":"a.rs"}"#),
            KernelLoopVerdict::Halt { .. }
        ));
        assert_eq!(executor.loop_breaks(), 1);
    }

    #[test]
    fn executor_preserves_specific_termination_when_finishing() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(executor.start_round(), 1);
        assert_eq!(executor.start_round(), 2);
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
            executor.permit_run(false, Duration::from_secs(4), Duration::from_secs(10)),
            KernelRunPermit::Proceed {
                remaining: Duration::from_secs(6)
            }
        );
        assert_eq!(
            executor.permit_run(true, Duration::from_secs(10), Duration::from_secs(10)),
            KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded)
        );
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::DeadlineExceeded)
        );
    }

    #[test]
    fn executor_run_permit_records_user_cancellation() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(
            executor.permit_run(true, Duration::from_secs(1), Duration::from_secs(10)),
            KernelRunPermit::Halt(KernelRunTermination::UserCancelled)
        );
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::UserCancelled)
        );
    }

    #[test]
    fn executor_tool_attempt_budget_counts_rejected_attempt() {
        let mut executor = KernelExecutorState::new();
        assert_eq!(
            executor.permit_tool_attempt(2),
            KernelToolAttemptPermit::Proceed { attempt: 1 }
        );
        assert_eq!(
            executor.permit_tool_attempt(2),
            KernelToolAttemptPermit::Proceed { attempt: 2 }
        );
        assert_eq!(
            executor.permit_tool_attempt(2),
            KernelToolAttemptPermit::Halt {
                attempted: 3,
                limit: 2
            }
        );
        assert_eq!(executor.tool_attempts(), 3);
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::ToolCallBudgetExceeded)
        );
    }
}
