//! UI/headless 共用的外层 Agent executor 状态所有者。
//!
//! 本层不直接执行外部 IO：adapter 负责 Provider、数据库、事件和工具执行；这里统一持有
//! 单调运行时钟、单轮路由、工具循环治理与唯一终止原因，避免跨轮状态在不同 adapter
//! 中独立装配。

use crate::agent::acceptance::AcceptanceReport;
use crate::agent::agent_kernel::{
    decide_stop_candidate, KernelRunState, KernelRunTermination, KernelStopDecision,
};
use crate::agent::kernel_loop::{
    KernelBudgetVerdict, KernelLoopGovernor, KernelLoopVerdict, KernelRoundCounters,
    KernelRoundDecision, KernelRoundInput, KernelRoundRouter, KernelToolBudgetGate,
    KERNEL_MAX_CONTINUATION_ROUNDS, KERNEL_MAX_EMPTY_ROUNDS, KERNEL_MAX_FAKE_CALL_CORRECTIONS,
    KERNEL_MAX_INTERRUPT_RETRY_ROUNDS, KERNEL_MAX_LOOP_BREAKS, KERNEL_MAX_STREAM_REPLAYS,
};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

pub const KERNEL_EXECUTOR_SNAPSHOT_VERSION: u32 = 1;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KernelExecutorLimits {
    pub wall_time_ms: u64,
    pub round_limit: Option<u64>,
    pub tool_attempt_limit: Option<u64>,
    pub remediation_limit: usize,
}

impl Default for KernelExecutorLimits {
    fn default() -> Self {
        Self {
            wall_time_ms: u64::MAX,
            round_limit: None,
            tool_attempt_limit: None,
            remediation_limit: usize::MAX,
        }
    }
}

/// 跨 adapter 稳定的 executor 最终快照，可直接写入桌面 run event 或 headless trajectory。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct KernelExecutorSnapshot {
    pub schema_version: u32,
    pub limits: KernelExecutorLimits,
    pub steps: u64,
    pub tool_attempts: u64,
    pub remediation_rounds: usize,
    pub loop_breaks: usize,
    pub round_counters: KernelRoundCounters,
    pub termination_reason: String,
    pub failure_taxonomy: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct KernelExecutorState {
    limits: KernelExecutorLimits,
    rounds: KernelRoundRouter,
    tools: KernelLoopGovernor,
    run: KernelRunState,
    completed_rounds: u64,
    tool_attempts: u64,
    remediation_rounds: usize,
}

/// UI/headless 共用的 Provider 外层循环壳。
///
/// 它把单调时钟与 executor 状态绑定在一起，使 adapter 不再自行计算并传入 elapsed，
/// 从而保证 wall-time、取消与回合上限始终在同一个 Provider 边界原子裁决。Provider、
/// 工具、事件和数据库仍由 adapter 实现；后续 IO 端口迁移以此类型为唯一循环所有者。
#[derive(Debug)]
pub struct KernelIoRunLoop {
    executor: KernelExecutorState,
    clock: KernelIoClock,
}

/// IO adapter 的只读运行时钟视图。端口可在工具结果等轮内安全点生成 checkpoint，
/// 但不能改变起点、累计耗时或把自算 elapsed 注入治理裁决。
#[derive(Clone, Debug)]
pub struct KernelIoClock {
    started: Instant,
    elapsed_before_start: Duration,
}

pub const KERNEL_EXECUTOR_CHECKPOINT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KernelCheckpointSafePoint {
    ProviderBoundary,
    ToolResult,
}

/// 活跃 run-loop 的版本化恢复契约。与 final snapshot 不同，它保留 router/governor 的
/// 完整内部状态；恢复时会把进程停止期间的墙钟时间计入预算，避免重启刷新 deadline。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct KernelExecutorCheckpoint {
    pub schema_version: u32,
    pub checkpointed_at_ms: u64,
    pub elapsed_ms: u64,
    state: KernelExecutorState,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct KernelExecutorCheckpointEnvelope {
    #[serde(flatten)]
    checkpoint: KernelExecutorCheckpoint,
    safe_point: KernelCheckpointSafePoint,
}

pub fn executor_checkpoint_payload(
    checkpoint: KernelExecutorCheckpoint,
    safe_point: KernelCheckpointSafePoint,
) -> Result<serde_json::Value, String> {
    serde_json::to_value(KernelExecutorCheckpointEnvelope {
        checkpoint,
        safe_point,
    })
    .map_err(|error| error.to_string())
}

pub fn restore_executor_checkpoint_payload(
    payload: serde_json::Value,
) -> Result<(KernelIoRunLoop, KernelCheckpointSafePoint), String> {
    let envelope: KernelExecutorCheckpointEnvelope =
        serde_json::from_value(payload).map_err(|error| error.to_string())?;
    let run_loop = KernelIoRunLoop::restore(envelope.checkpoint)?;
    Ok((run_loop, envelope.safe_point))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelIoRoundControl {
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelIoRunExit {
    AdapterStopped,
    Halted(KernelRunTermination),
}

/// 单一 IO run-loop 的 adapter 端口。实现方只负责一轮 Provider/工具/事件 IO；循环、
/// 单调时钟、安全点和终态吸收由 [`KernelIoRunLoop::run`] 统一负责。
pub trait KernelIoPort {
    type Error;

    fn cancelled(&mut self) -> bool;

    /// 每个后续 Provider 边界前的安全点。run-loop 负责生成可信 checkpoint 并调用，
    /// adapter 只负责持久化，不能遗漏生成时机或自行拼装 elapsed。
    fn persist_checkpoint(
        &mut self,
        _checkpoint: KernelExecutorCheckpoint,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn run_round<'a>(
        &'a mut self,
        executor: &'a mut KernelExecutorState,
        clock: &'a KernelIoClock,
        round: u64,
        remaining: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<KernelIoRoundControl, Self::Error>> + Send + 'a>>;
}

impl KernelIoClock {
    pub fn elapsed(&self) -> Duration {
        self.elapsed_before_start
            .saturating_add(self.started.elapsed())
    }

    pub fn checkpoint(&self, executor: &KernelExecutorState) -> KernelExecutorCheckpoint {
        KernelExecutorCheckpoint {
            schema_version: KERNEL_EXECUTOR_CHECKPOINT_VERSION,
            checkpointed_at_ms: unix_time_ms(),
            elapsed_ms: self.elapsed().as_millis().min(u64::MAX as u128) as u64,
            state: executor.clone(),
        }
    }
}

impl KernelIoRunLoop {
    pub fn new(limits: KernelExecutorLimits) -> Self {
        Self::with_started(limits, Instant::now())
    }

    pub fn with_started(limits: KernelExecutorLimits, started: Instant) -> Self {
        Self {
            executor: KernelExecutorState::with_limits(limits),
            clock: KernelIoClock {
                started,
                elapsed_before_start: Duration::ZERO,
            },
        }
    }

    /// Provider 请求前的唯一循环入口。adapter 只能提供当前取消信号，不能注入自行计算的
    /// deadline/elapsed；单调耗时由循环壳在裁决瞬间采样。
    pub fn begin_next_round(&mut self, cancelled: bool) -> KernelRunPermit {
        let elapsed = self.elapsed();
        self.executor.begin_round(cancelled, elapsed)
    }

    /// 在 Provider 边界原子执行“持久化当前安全点 → 轮次裁决”。持久化失败时不会推进
    /// completed_rounds，也不会给 adapter 返回可发起外部请求的 permit。
    pub fn begin_persisted_round<E>(
        &mut self,
        cancelled: bool,
        persist: impl FnOnce(KernelExecutorCheckpoint) -> Result<(), E>,
    ) -> Result<KernelRunPermit, E> {
        persist(self.checkpoint())?;
        Ok(self.begin_next_round(cancelled))
    }

    pub fn elapsed(&self) -> Duration {
        self.clock.elapsed()
    }

    pub fn checkpoint(&self) -> KernelExecutorCheckpoint {
        self.clock.checkpoint(&self.executor)
    }

    pub fn restore(checkpoint: KernelExecutorCheckpoint) -> Result<Self, String> {
        if checkpoint.schema_version != KERNEL_EXECUTOR_CHECKPOINT_VERSION {
            return Err(format!(
                "不支持 executor checkpoint schema_version={}（当前={}）",
                checkpoint.schema_version, KERNEL_EXECUTOR_CHECKPOINT_VERSION
            ));
        }
        let now_ms = unix_time_ms();
        if now_ms < checkpoint.checkpointed_at_ms {
            return Err("系统时钟早于 executor checkpoint，拒绝恢复墙钟预算".into());
        }
        checkpoint.state.validate_checkpoint_state()?;
        let total_elapsed_ms = checkpoint
            .elapsed_ms
            .saturating_add(now_ms - checkpoint.checkpointed_at_ms);
        Ok(Self {
            executor: checkpoint.state,
            clock: KernelIoClock {
                started: Instant::now(),
                elapsed_before_start: Duration::from_millis(total_elapsed_ms),
            },
        })
    }

    /// 驱动 adapter 直到其主动停止或 executor 锁定终态。任何下一轮 Provider IO 都必须先
    /// 经过 `begin_next_round`，因此回合上限、deadline、取消和既有终态无法被端口绕过。
    pub async fn run<P: KernelIoPort>(
        &mut self,
        port: &mut P,
    ) -> Result<KernelIoRunExit, P::Error> {
        loop {
            let cancelled = port.cancelled();
            let permit = self.begin_persisted_round(cancelled, |checkpoint| {
                port.persist_checkpoint(checkpoint)
            })?;
            let (round, remaining) = match permit {
                KernelRunPermit::Proceed { round, remaining } => (round, remaining),
                KernelRunPermit::Halt(reason) => {
                    return Ok(KernelIoRunExit::Halted(reason));
                }
            };
            match port
                .run_round(&mut self.executor, &self.clock, round, remaining)
                .await?
            {
                KernelIoRoundControl::Continue => {}
                KernelIoRoundControl::Stop => return Ok(KernelIoRunExit::AdapterStopped),
            }
        }
    }
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

impl std::ops::Deref for KernelIoRunLoop {
    type Target = KernelExecutorState;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

impl std::ops::DerefMut for KernelIoRunLoop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.executor
    }
}

impl KernelExecutorState {
    fn new() -> Self {
        Self::default()
    }

    fn with_limits(limits: KernelExecutorLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    fn validate_checkpoint_state(&self) -> Result<(), String> {
        if self
            .limits
            .round_limit
            .is_some_and(|limit| self.completed_rounds > limit)
        {
            return Err("executor checkpoint 的 completed_rounds 超过冻结上限".into());
        }
        if self
            .limits
            .tool_attempt_limit
            .is_some_and(|limit| self.tool_attempts > limit.saturating_add(1))
        {
            return Err("executor checkpoint 的 tool_attempts 超过可达范围".into());
        }
        if self.remediation_rounds > self.limits.remediation_limit {
            return Err("executor checkpoint 的 remediation_rounds 超过冻结上限".into());
        }
        if self.tools.loop_breaks() > KERNEL_MAX_LOOP_BREAKS.saturating_add(1) {
            return Err("executor checkpoint 的 loop_breaks 超过可达范围".into());
        }
        let counters = self.rounds.counters();
        if counters.empty_rounds > KERNEL_MAX_EMPTY_ROUNDS
            || counters.stream_replays > KERNEL_MAX_STREAM_REPLAYS
            || counters.interrupted_rounds > KERNEL_MAX_INTERRUPT_RETRY_ROUNDS
            || counters.continuation_rounds > KERNEL_MAX_CONTINUATION_ROUNDS
            || counters.fake_corrections > KERNEL_MAX_FAKE_CALL_CORRECTIONS
        {
            return Err("executor checkpoint 的 round counters 超过可达范围".into());
        }
        Ok(())
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
    fn begin_round(
        &mut self,
        cancelled: bool,
        elapsed: Duration,
    ) -> KernelRunPermit {
        let deadline = Duration::from_millis(self.limits.wall_time_ms);
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
            schema_version: KERNEL_EXECUTOR_SNAPSHOT_VERSION,
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

    struct ScriptedIoPort {
        rounds: Vec<u64>,
        checkpoints: Vec<KernelExecutorCheckpoint>,
        boundary_checkpoints: Vec<KernelExecutorCheckpoint>,
        stop_after: Option<u64>,
        cancelled: bool,
    }

    impl KernelIoPort for ScriptedIoPort {
        type Error = String;

        fn cancelled(&mut self) -> bool {
            self.cancelled
        }

        fn persist_checkpoint(
            &mut self,
            checkpoint: KernelExecutorCheckpoint,
        ) -> Result<(), Self::Error> {
            self.boundary_checkpoints.push(checkpoint);
            Ok(())
        }

        fn run_round<'a>(
            &'a mut self,
            executor: &'a mut KernelExecutorState,
            clock: &'a KernelIoClock,
            round: u64,
            _remaining: Duration,
        ) -> Pin<Box<dyn Future<Output = Result<KernelIoRoundControl, Self::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                self.rounds.push(round);
                self.checkpoints.push(clock.checkpoint(executor));
                Ok(if self.stop_after == Some(round) {
                    KernelIoRoundControl::Stop
                } else {
                    KernelIoRoundControl::Continue
                })
            })
        }
    }

    #[tokio::test]
    async fn io_run_loop_drives_one_port_until_adapter_stop() {
        let mut run_loop = KernelIoRunLoop::new(KernelExecutorLimits {
            wall_time_ms: 60_000,
            round_limit: Some(5),
            tool_attempt_limit: None,
            remediation_limit: 0,
        });
        let mut port = ScriptedIoPort {
            rounds: Vec::new(),
            checkpoints: Vec::new(),
            boundary_checkpoints: Vec::new(),
            stop_after: Some(2),
            cancelled: false,
        };

        assert_eq!(
            run_loop.run(&mut port).await.unwrap(),
            KernelIoRunExit::AdapterStopped
        );
        assert_eq!(port.rounds, vec![1, 2]);
        assert_eq!(port.checkpoints.len(), 2);
        assert_eq!(port.checkpoints[0].state.completed_rounds, 1);
        assert_eq!(port.checkpoints[1].state.completed_rounds, 2);
        assert_eq!(port.boundary_checkpoints.len(), 2);
        assert_eq!(port.boundary_checkpoints[0].state.completed_rounds, 0);
        assert_eq!(port.boundary_checkpoints[1].state.completed_rounds, 1);
    }

    #[tokio::test]
    async fn io_run_loop_halts_before_port_io_at_frozen_limit() {
        let mut run_loop = KernelIoRunLoop::new(KernelExecutorLimits {
            wall_time_ms: 60_000,
            round_limit: Some(2),
            tool_attempt_limit: None,
            remediation_limit: 0,
        });
        let mut port = ScriptedIoPort {
            rounds: Vec::new(),
            checkpoints: Vec::new(),
            boundary_checkpoints: Vec::new(),
            stop_after: None,
            cancelled: false,
        };

        assert_eq!(
            run_loop.run(&mut port).await.unwrap(),
            KernelIoRunExit::Halted(KernelRunTermination::MaxStepsExceeded)
        );
        assert_eq!(port.rounds, vec![1, 2]);
        assert_eq!(port.boundary_checkpoints.len(), 3);
        assert_eq!(port.boundary_checkpoints[0].state.completed_rounds, 0);
        assert_eq!(port.boundary_checkpoints[2].state.completed_rounds, 2);
    }

    #[test]
    fn persisted_round_does_not_advance_when_checkpoint_fails() {
        let mut run_loop = KernelIoRunLoop::new(KernelExecutorLimits::default());
        let error = run_loop
            .begin_persisted_round(false, |_| Err::<(), _>("checkpoint rejected"))
            .unwrap_err();
        assert_eq!(error, "checkpoint rejected");

        let permit = run_loop
            .begin_persisted_round(false, |_| Ok::<(), String>(()))
            .unwrap();
        assert!(matches!(permit, KernelRunPermit::Proceed { round: 1, .. }));
    }

    #[test]
    fn active_checkpoint_round_trips_router_governor_and_counters() {
        let mut run_loop = KernelIoRunLoop::new(KernelExecutorLimits {
            wall_time_ms: 60_000,
            round_limit: Some(4),
            tool_attempt_limit: Some(20),
            remediation_limit: 2,
        });
        assert!(matches!(
            run_loop.begin_next_round(false),
            KernelRunPermit::Proceed { round: 1, .. }
        ));
        let empty = KernelRoundInput {
            text: "",
            has_reasoning: false,
            truncated: false,
            interrupted: false,
            has_native_tool_calls: false,
        };
        assert!(matches!(
            run_loop.decide_round(&empty).control,
            KernelRoundControl::RetryEmpty { .. }
        ));
        for _ in 0..4 {
            assert!(matches!(
                run_loop.begin_tool_attempt("read_file", r#"{"path":"a.rs"}"#),
                KernelToolAttemptDecision::Observed { .. }
            ));
        }

        let encoded = serde_json::to_string(&run_loop.checkpoint()).unwrap();
        let checkpoint: KernelExecutorCheckpoint = serde_json::from_str(&encoded).unwrap();
        let mut restored = KernelIoRunLoop::restore(checkpoint).unwrap();

        assert!(matches!(
            restored.begin_next_round(false),
            KernelRunPermit::Proceed { round: 2, .. }
        ));
        assert!(matches!(
            restored.decide_round(&empty).control,
            KernelRoundControl::StopEmpty { .. }
        ));
        assert!(matches!(
            restored.begin_tool_attempt("read_file", r#"{"path":"a.rs"}"#),
            KernelToolAttemptDecision::Halt {
                reason: KernelRunTermination::EmptyRoundsExhausted,
                ..
            }
        ));
    }

    #[test]
    fn active_checkpoint_rejects_unknown_schema() {
        let run_loop = KernelIoRunLoop::new(KernelExecutorLimits::default());
        let mut checkpoint = run_loop.checkpoint();
        checkpoint.schema_version += 1;
        assert!(KernelIoRunLoop::restore(checkpoint)
            .unwrap_err()
            .contains("schema_version"));
    }

    #[test]
    fn checkpoint_envelope_requires_a_known_safe_point() {
        let run_loop = KernelIoRunLoop::new(KernelExecutorLimits::default());
        let payload = executor_checkpoint_payload(
            run_loop.checkpoint(),
            KernelCheckpointSafePoint::ToolResult,
        )
        .unwrap();
        let (_, safe_point) = restore_executor_checkpoint_payload(payload.clone()).unwrap();
        assert_eq!(safe_point, KernelCheckpointSafePoint::ToolResult);

        for replacement in [Some(serde_json::json!("mid_tool")), None] {
            let mut invalid = payload.clone();
            match replacement {
                Some(value) => invalid["safe_point"] = value,
                None => {
                    invalid.as_object_mut().unwrap().remove("safe_point");
                }
            }
            assert!(restore_executor_checkpoint_payload(invalid).is_err());
        }
    }

    #[test]
    fn active_checkpoint_rejects_impossible_persisted_counters() {
        let run_loop = KernelIoRunLoop::new(KernelExecutorLimits {
            wall_time_ms: 60_000,
            round_limit: Some(2),
            tool_attempt_limit: Some(3),
            remediation_limit: 1,
        });
        let base = serde_json::to_value(run_loop.checkpoint()).unwrap();
        for (path, value, expected) in [
            ("/state/completed_rounds", 3_u64, "completed_rounds"),
            ("/state/tool_attempts", 5_u64, "tool_attempts"),
            ("/state/remediation_rounds", 2_u64, "remediation_rounds"),
            ("/state/rounds/empty_rounds", 3_u64, "round counters"),
            ("/state/tools/loop_breaks", 4_u64, "loop_breaks"),
        ] {
            let mut tampered = base.clone();
            *tampered.pointer_mut(path).expect("checkpoint field") = serde_json::json!(value);
            let checkpoint: KernelExecutorCheckpoint = serde_json::from_value(tampered).unwrap();
            let error = KernelIoRunLoop::restore(checkpoint).unwrap_err();
            assert!(error.contains(expected), "{path}: {error}");
        }
    }

    #[test]
    fn restored_elapsed_cannot_refresh_wall_time_budget() {
        let run_loop = KernelIoRunLoop::new(KernelExecutorLimits {
            wall_time_ms: 60_000,
            ..KernelExecutorLimits::default()
        });
        let mut checkpoint = run_loop.checkpoint();
        checkpoint.elapsed_ms = 120_000;
        let mut restored = KernelIoRunLoop::restore(checkpoint).unwrap();
        assert_eq!(
            restored.begin_next_round(false),
            KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded)
        );
    }

    #[test]
    fn active_checkpoint_never_serializes_raw_tool_arguments() {
        let secret = "sk-secret-checkpoint-value";
        let mut run_loop = KernelIoRunLoop::new(KernelExecutorLimits::default());
        assert!(matches!(
            run_loop.begin_tool_attempt(
                "write_file",
                &format!(r#"{{"path":"a.txt","content":"{secret}"}}"#),
            ),
            KernelToolAttemptDecision::Observed { .. }
        ));

        let encoded = serde_json::to_string(&run_loop.checkpoint()).unwrap();
        assert!(!encoded.contains(secret), "checkpoint 泄露了原始工具参数");
        assert!(encoded.contains("sha256:"), "checkpoint 缺少稳定调用指纹");
    }

    #[test]
    fn io_run_loop_owns_monotonic_wall_time_and_absorbs_halt() {
        let limits = KernelExecutorLimits {
            wall_time_ms: 5,
            round_limit: Some(3),
            tool_attempt_limit: Some(2),
            remediation_limit: 1,
        };
        let mut run_loop =
            KernelIoRunLoop::with_started(limits, Instant::now() - Duration::from_millis(20));

        assert_eq!(
            run_loop.begin_next_round(false),
            KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded)
        );
        assert_eq!(
            run_loop.begin_next_round(true),
            KernelRunPermit::Halt(KernelRunTermination::DeadlineExceeded)
        );
        assert_eq!(run_loop.completed_rounds(), 0);
    }

    #[test]
    fn io_run_loop_delegates_round_counter_and_cancel_priority() {
        let limits = KernelExecutorLimits {
            wall_time_ms: 60_000,
            round_limit: Some(2),
            tool_attempt_limit: None,
            remediation_limit: 0,
        };
        let mut run_loop = KernelIoRunLoop::new(limits);

        assert!(matches!(
            run_loop.begin_next_round(false),
            KernelRunPermit::Proceed { round: 1, .. }
        ));
        assert_eq!(
            run_loop.begin_next_round(true),
            KernelRunPermit::Halt(KernelRunTermination::UserCancelled)
        );
        assert_eq!(run_loop.completed_rounds(), 1);
    }

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
            executor.begin_round(false, Duration::ZERO),
            KernelRunPermit::Proceed { round: 1, .. }
        ));
        assert!(matches!(
            executor.begin_round(false, Duration::ZERO),
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
        let mut executor = KernelExecutorState::with_limits(KernelExecutorLimits {
            wall_time_ms: 10_000,
            ..KernelExecutorLimits::default()
        });
        assert_eq!(
            executor.begin_round(false, Duration::from_secs(4)),
            KernelRunPermit::Proceed {
                round: 1,
                remaining: Duration::from_secs(6)
            }
        );
        assert_eq!(
            executor.begin_round(true, Duration::from_secs(10)),
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
            executor.begin_round(true, Duration::from_secs(1)),
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
            wall_time_ms: u64::MAX,
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
        assert_eq!(json["schema_version"], KERNEL_EXECUTOR_SNAPSHOT_VERSION);
        assert_eq!(json["limits"]["wall_time_ms"], u64::MAX);
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
            executor.begin_round(false, Duration::ZERO),
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
            executor.begin_round(true, Duration::from_secs(20)),
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
            executor.begin_round(false, Duration::ZERO),
            KernelRunPermit::Proceed { round: 1, .. }
        ));
        assert_eq!(
            executor.begin_round(true, Duration::from_secs(20)),
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
