//! UI/headless 共用的外层 Agent executor 状态所有者。
//!
//! 本层仍保持纯状态、无 IO：adapter 负责 Provider、数据库、事件和工具执行；这里统一持有
//! 单轮路由、工具循环治理与唯一终止原因，避免三套跨轮状态在不同 adapter 中独立装配。

use crate::agent::agent_kernel::{KernelRunState, KernelRunTermination};
use crate::agent::kernel_loop::{
    KernelLoopGovernor, KernelLoopVerdict, KernelRoundDecision, KernelRoundInput,
    KernelRoundRouter,
};

#[derive(Clone, Debug, Default)]
pub struct KernelExecutorState {
    rounds: KernelRoundRouter,
    tools: KernelLoopGovernor,
    run: KernelRunState,
}

impl KernelExecutorState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decide_round(&mut self, input: &KernelRoundInput<'_>) -> KernelRoundDecision {
        self.rounds.decide(input)
    }

    pub fn observe_tool(&mut self, tool: &str, args: &str) -> KernelLoopVerdict {
        self.tools.observe(tool, args)
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

    pub fn finish(
        &mut self,
        completed_steps: u64,
        round_limit: u64,
    ) -> Option<KernelRunTermination> {
        self.run.finish(completed_steps, round_limit)
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
        executor.terminate(KernelRunTermination::ToolCallBudgetExceeded);
        assert_eq!(
            executor.finish(10, 10),
            Some(KernelRunTermination::ToolCallBudgetExceeded)
        );
        assert_eq!(
            executor.termination(),
            Some(KernelRunTermination::ToolCallBudgetExceeded)
        );
    }
}
