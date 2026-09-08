//! AgentKernel 共享循环治理：工具循环检测、预算门控、轮级路由。
//!
//! 纯策略、无 IO、无 Tauri 依赖：UI（chat.rs）与 headless（headless_driver.rs）各自持有实例，
//! 在原有循环位置调用；效果代码（事件、DB 写入、continue/break）留在各自 adapter 逐字保留。
//! 阈值与文案从 chat.rs 主循环逐字搬入，保证 UI 行为零变化。

use crate::agent::tools::strip_tool_calls;

// ── 工具循环检测阈值（对齐 chat.rs 原常量，UI 与 headless 共用） ─────────────────
/// 连续相同调用（同工具名+同参数）达到该次数即判定打转——重复调用必得相同结果，
/// 低于 DashScope 服务端 "Repetitive tool calls detected" 阈值，客户端先断循环防服务端 400
pub const KERNEL_TOOL_CALL_LOOP_THRESHOLD: usize = 5;
/// 连续同名调用（不管参数）达到该次数判定参数抖动循环（模型反复调同一工具换参数试探）
pub const KERNEL_TOOL_NAME_STAGNATION_THRESHOLD: usize = 8;
/// 每轮工具调用软上限：仅停滞信号（连续 3 次相同调用）时生效，长任务后期收尾阶段重复
/// 同一验证命令不必等满 5 次
pub const KERNEL_MAX_TOOL_CALLS_PER_TURN: usize = 100;
/// 每轮工具调用硬上限：无条件中止，防参数变化逃逸重复检测
pub const KERNEL_MAX_TOOL_CALLS_HARD: usize = 1000;
/// 循环检测命中后注入纠正提示的轮数上限：模型收到提示仍循环时最多打断两次，
/// 之后直接收尾（防"纠正-循环-再纠正"空转）
pub const KERNEL_MAX_LOOP_BREAKS: usize = 2;

// ── 轮级门控阈值（对齐 chat.rs 原常量） ─────────────────────────────────────────
/// 输出截断续写次数上限：模型被 max_tokens 截断后自动追加"请继续"续写，防无限啰嗦
pub const KERNEL_MAX_CONTINUATION_ROUNDS: usize = 8;
/// 空响应重试上限：模型连续多轮输出为空（服务端静默失败/异常截断）时最多重试
/// 两次即收尾提示，防止进入无限空轮循环导致界面长时间无输出看起来卡死
pub const KERNEL_MAX_EMPTY_ROUNDS: usize = 2;
/// 连接中断自动续写次数上限：网络问题重试 3 次无意义（多为本地代理/网络故障），
/// 超过后收尾并明确提示，避免无限续写空转
pub const KERNEL_MAX_INTERRUPT_RETRY_ROUNDS: usize = 3;
/// 产出前中断重放次数上限：流在输出任何内容前即中断（服务端断流/代理重置）时，
/// 用冻结请求原样重发（对齐 DeepSeek-Reasonix 冻结请求重放机制）；连续 5 次 0 产出
/// 中断多为本地网络故障，超过后走下方续写收尾
pub const KERNEL_MAX_STREAM_REPLAYS: usize = 5;
/// "叙述式假调用"纠正次数上限：模型在正文里写"已调用工具"却不输出标记时，
/// 自动注入纠正提示继续（历史格式污染导致模型模仿，纠正后重走标记协议）
pub const KERNEL_MAX_FAKE_CALL_CORRECTIONS: usize = 3;

/// 工具循环检测裁决：Proceed 表示继续执行；Halt 表示命中循环需打断。
///
/// `final_halt` 为 true 时 loop_breaks 已超上限，adapter 应直接收尾（exhausted=true）；
/// 否则 adapter 注入 `corrective_hint` 让模型换方案，下一轮继续观察。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelLoopVerdict {
    Proceed,
    Halt {
        corrective_hint: Option<String>,
        final_halt: bool,
        repeat: usize,
        same_name: usize,
        turn_calls: usize,
    },
}

/// 工具循环检测状态机：跨轮保持计数，observe() 每次工具调用时推进。
///
/// 注意：`turn_tool_calls` 现状是从不重置的任务级累计（chat.rs 注释标"每轮"但实为累计）——
/// 忠实复刻，不顺手修复，避免行为变化。
#[derive(Clone, Debug, Default)]
pub struct KernelLoopGovernor {
    turn_tool_calls: usize,
    last_tool_call_key: Option<String>,
    tool_call_repeat: usize,
    last_tool_name: Option<String>,
    same_name_streak: usize,
    loop_breaks: usize,
}

impl KernelLoopGovernor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 每次工具调用前调用：推进计数，返回裁决。
    ///
    /// halt 条件逐字对齐 chat.rs 5392-5394：
    /// stuck(≥5 相同 name+args 或 ≥8 同名) || (turn_calls>100 && repeat≥3) || turn_calls>1000
    pub fn observe(&mut self, tool: &str, args: &str) -> KernelLoopVerdict {
        self.turn_tool_calls += 1;
        let call_key = format!("{tool}|{args}");
        if self.last_tool_call_key.as_deref() == Some(call_key.as_str()) {
            self.tool_call_repeat += 1;
        } else {
            self.last_tool_call_key = Some(call_key);
            self.tool_call_repeat = 1;
        }
        if self.last_tool_name.as_deref() == Some(tool) {
            self.same_name_streak += 1;
        } else {
            self.last_tool_name = Some(tool.to_string());
            self.same_name_streak = 1;
        }
        let stuck = self.tool_call_repeat >= KERNEL_TOOL_CALL_LOOP_THRESHOLD
            || self.same_name_streak >= KERNEL_TOOL_NAME_STAGNATION_THRESHOLD;
        let halt = stuck
            || (self.turn_tool_calls > KERNEL_MAX_TOOL_CALLS_PER_TURN
                && self.tool_call_repeat >= 3)
            || self.turn_tool_calls > KERNEL_MAX_TOOL_CALLS_HARD;
        if !halt {
            return KernelLoopVerdict::Proceed;
        }
        self.loop_breaks += 1;
        let final_halt = self.loop_breaks > KERNEL_MAX_LOOP_BREAKS;
        let corrective_hint = if final_halt {
            None
        } else {
            Some(format!(
                "（系统检测到工具调用循环：工具 {tool} 已连续重复调用 {} 次（连续同名 {} 次，本轮共 {} 次调用）。重复执行只会得到相同结果。请立即停止当前路径，改用其他工具/思路推进；若确实无法推进，请直接给出结论总结与所需条件。）",
                self.tool_call_repeat, self.same_name_streak, self.turn_tool_calls
            ))
        };
        KernelLoopVerdict::Halt {
            corrective_hint,
            final_halt,
            repeat: self.tool_call_repeat,
            same_name: self.same_name_streak,
            turn_calls: self.turn_tool_calls,
        }
    }

    /// 当前 loop_breaks 计数：供 KernelToolBudgetGate 与 adapter 日志使用。
    pub fn loop_breaks(&self) -> usize {
        self.loop_breaks
    }

    /// 任务级累计工具调用数（不重置，忠实复刻 chat.rs 现状）。
    pub fn turn_tool_calls(&self) -> usize {
        self.turn_tool_calls
    }
}

/// 工具预算裁决：Proceed 表示未达上限；Extend 表示可延长预算；Halt 表示必须停止。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelBudgetVerdict {
    Proceed,
    Extend { new_limit: usize },
    Halt,
}

/// 工具预算门控：判断是否达到上限以及是否可延长。
///
/// 内部调用已共享的 `crate::agent::governance::extend_tool_budget`（chat.rs 5424 已用）。
/// `used` = tool_runs.len() + pending.len()（UI）或 tool_calls 累计（headless）。
pub struct KernelToolBudgetGate;

impl KernelToolBudgetGate {
    pub fn check(
        max_tool_rounds: usize,
        used: usize,
        recent_successes: usize,
        loop_breaks: usize,
        extensions: usize,
    ) -> KernelBudgetVerdict {
        if used < max_tool_rounds {
            return KernelBudgetVerdict::Proceed;
        }
        match crate::agent::governance::extend_tool_budget(
            max_tool_rounds,
            recent_successes,
            loop_breaks,
            extensions,
        ) {
            Some(new_limit) => KernelBudgetVerdict::Extend { new_limit },
            None => KernelBudgetVerdict::Halt,
        }
    }
}

/// 轮级路由输入：adapter 从 turn 解析结果构造。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelRoundInput<'a> {
    pub text: &'a str,
    pub has_reasoning: bool,
    pub truncated: bool,
    pub interrupted: bool,
    pub has_native_tool_calls: bool,
}

/// 轮级路由动作：adapter 按动作执行效果（continue/break/落穿后续 UI 专属门）。
///
/// 注意：InterruptedNote 是非终态动作，adapter 追加注记后继续执行后续门控（对齐 chat.rs
/// 6253 不 continue 的设计）。其他动作均为终态，adapter 执行后 continue/break。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelRoundAction {
    /// 继续后续 UI 专属门（pending-action/action-commitment/acceptance 等）
    Proceed,
    /// 空轮重试：注入纠正提示，下一轮继续
    RetryEmpty { hint: String },
    /// 空轮耗尽：追加注记后收尾
    StopEmpty { note: String },
    /// 冻结重放：流在输出任何内容前中断，原样重发
    ReplayFrozen,
    /// 连接中断续写：保留已收内容，从断点继续
    ContinueInterrupted {
        continuation_text: String,
        reasoning_only: bool,
    },
    /// 中断耗尽注记：追加后落穿后续门（对齐 chat.rs 6253 不 continue）
    InterruptedNote { note: String },
    /// 截断续写：保留已有内容，从截断处继续
    ContinueTruncated {
        continuation_text: String,
        reasoning_only: bool,
    },
    /// 假调用纠正：注入纠正提示继续
    CorrectFakeCall {
        correction_text: String,
        hint: String,
    },
}

/// 轮级路由状态机：跨轮保持计数，route() 每轮调用一次。
///
/// 优先级逐字对齐 chat.rs 6205-6276：空轮 → 冻结重放 → 中断续写 → 中断耗尽注记（落穿）
/// → 截断续写 → fake-call。
#[derive(Clone, Debug, Default)]
pub struct KernelRoundRouter {
    empty_rounds: usize,
    stream_replays: usize,
    interrupted_rounds: usize,
    continuation_rounds: usize,
    fake_corrections: usize,
}

impl KernelRoundRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 每轮 turn 解析后调用：返回动作序列，adapter 按序执行。
    ///
    /// 非终态动作（InterruptedNote）允许后续动作继续评估（对齐 chat.rs 6253 不 continue
    /// 的设计：中断耗尽注记后仍检查截断/fake-call）。终态动作后不再评估。
    pub fn route(&mut self, input: &KernelRoundInput) -> Vec<KernelRoundAction> {
        let mut actions = Vec::new();
        let text_empty = input.text.trim().is_empty();
        // 空轮：text 空且非截断非中断且无工具调用（工具调用响应可能无正文，属正常）
        if text_empty && !input.truncated && !input.interrupted && !input.has_native_tool_calls {
            self.empty_rounds += 1;
            if self.empty_rounds >= KERNEL_MAX_EMPTY_ROUNDS {
                actions.push(KernelRoundAction::StopEmpty {
                    note: "\n\n> ⚠️ 模型连续多次未输出内容（可能服务端异常），任务已中止；可重新发送指令重试。".to_string(),
                });
                return actions;
            }
            actions.push(KernelRoundAction::RetryEmpty {
                hint: "（系统提示：你上一轮未输出任何内容，请重新生成完整回复；若任务已完成请直接给出结论，若需继续请输出工具调用标记。）".to_string(),
            });
            return actions;
        }
        // 冻结重放：中断且 text 空且无工具调用且重放次数未耗尽
        if input.interrupted
            && text_empty
            && !input.has_native_tool_calls
            && self.stream_replays < KERNEL_MAX_STREAM_REPLAYS
        {
            self.stream_replays += 1;
            actions.push(KernelRoundAction::ReplayFrozen);
            return actions;
        }
        // 中断续写：中断且续写次数未耗尽
        if input.interrupted && self.interrupted_rounds < KERNEL_MAX_INTERRUPT_RETRY_ROUNDS {
            self.interrupted_rounds += 1;
            actions.push(KernelRoundAction::ContinueInterrupted {
                continuation_text: strip_tool_calls(input.text),
                reasoning_only: text_empty && input.has_reasoning,
            });
            return actions;
        }
        // 中断耗尽注记：落穿（不 return，继续评估截断/fake-call）
        if input.interrupted {
            actions.push(KernelRoundAction::InterruptedNote {
                note: "\n\n> ⚠️ 网络连续中断（自动续写多次仍未恢复），已保留以上内容；可重新发送指令重试。".to_string(),
            });
        }
        // 截断续写：截断且续写次数未耗尽
        if input.truncated && self.continuation_rounds < KERNEL_MAX_CONTINUATION_ROUNDS {
            self.continuation_rounds += 1;
            actions.push(KernelRoundAction::ContinueTruncated {
                continuation_text: strip_tool_calls(input.text),
                reasoning_only: text_empty && input.has_reasoning,
            });
            return actions;
        }
        // 假调用纠正：正文含"已调用工具/工具调用记录"且纠正次数未耗尽
        if (input.text.contains("已调用工具") || input.text.contains("工具调用记录"))
            && self.fake_corrections < KERNEL_MAX_FAKE_CALL_CORRECTIONS
        {
            self.fake_corrections += 1;
            actions.push(KernelRoundAction::CorrectFakeCall {
                correction_text: strip_tool_calls(input.text),
                hint: "（检测到你的回复中出现了\u{201c}已调用工具/工具调用记录\u{201d}等叙述，但未输出工具调用标记，系统未执行任何工具。如需调用工具，请输出【TOOL|工具名|JSON参数】标记行，一行一个；若任务已完成，请直接给出结论总结，不要写\u{201c}已调用工具\u{201d}之类的叙述。）".to_string(),
            });
            return actions;
        }
        // 无特殊动作：继续后续 UI 专属门
        if actions.is_empty() {
            actions.push(KernelRoundAction::Proceed);
        }
        actions
    }

    /// 当前各计数器状态：供测试与诊断使用。
    pub fn counters(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.empty_rounds,
            self.stream_replays,
            self.interrupted_rounds,
            self.continuation_rounds,
            self.fake_corrections,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_governor_proceeds_on_normal_calls() {
        let mut gov = KernelLoopGovernor::new();
        assert_eq!(gov.observe("read_file", "{\"path\":\"a.rs\"}"), KernelLoopVerdict::Proceed);
        assert_eq!(gov.observe("write_file", "{\"path\":\"b.rs\"}"), KernelLoopVerdict::Proceed);
        assert_eq!(gov.turn_tool_calls(), 2);
        assert_eq!(gov.loop_breaks(), 0);
    }

    #[test]
    fn loop_governor_halt_at_threshold_5_identical_calls() {
        let mut gov = KernelLoopGovernor::new();
        for _ in 0..4 {
            assert_eq!(gov.observe("read_file", "{\"path\":\"x\"}"), KernelLoopVerdict::Proceed);
        }
        let verdict = gov.observe("read_file", "{\"path\":\"x\"}");
        match verdict {
            KernelLoopVerdict::Halt { corrective_hint, final_halt, repeat, same_name, turn_calls } => {
                assert!(corrective_hint.is_some());
                assert!(!final_halt);
                assert_eq!(repeat, 5);
                assert_eq!(same_name, 5);
                assert_eq!(turn_calls, 5);
            }
            _ => panic!("expected Halt at 5th identical call"),
        }
        assert_eq!(gov.loop_breaks(), 1);
    }

    #[test]
    fn loop_governor_final_halt_after_max_loop_breaks() {
        let mut gov = KernelLoopGovernor::new();
        // 连续 5 次相同 → 第 5 次 halt（loop_breaks=1）
        for _ in 0..4 { gov.observe("read_file", "{\"path\":\"x\"}"); }
        let v1 = gov.observe("read_file", "{\"path\":\"x\"}");
        assert!(matches!(v1, KernelLoopVerdict::Halt { final_halt: false, .. }));
        assert_eq!(gov.loop_breaks(), 1);
        // 模拟 adapter 注入纠正后模型换方案：用不同参数调用（repeat 重置）
        gov.observe("write_file", "{\"path\":\"y\"}");
        // 模型又回到循环：连续 5 次相同 → 再次 halt（loop_breaks=2）
        for _ in 0..4 { gov.observe("read_file", "{\"path\":\"z\"}"); }
        let v2 = gov.observe("read_file", "{\"path\":\"z\"}");
        assert!(matches!(v2, KernelLoopVerdict::Halt { final_halt: false, .. }));
        assert_eq!(gov.loop_breaks(), 2);
        // 模拟再次纠正后模型仍循环：第 3 次 halt（loop_breaks=3 > MAX_LOOP_BREAKS=2 → final）
        gov.observe("write_file", "{\"path\":\"w\"}");
        for _ in 0..4 { gov.observe("read_file", "{\"path\":\"v\"}"); }
        let v3 = gov.observe("read_file", "{\"path\":\"v\"}");
        match v3 {
            KernelLoopVerdict::Halt { corrective_hint, final_halt, .. } => {
                assert!(corrective_hint.is_none());
                assert!(final_halt);
            }
            _ => panic!("expected final Halt"),
        }
        assert_eq!(gov.loop_breaks(), 3);
    }

    #[test]
    fn loop_governor_name_stagnation_at_8() {
        let mut gov = KernelLoopGovernor::new();
        // 连续同名但参数不同：第 8 次触发
        for i in 0..7 {
            assert_eq!(gov.observe("read_file", &format!("{{\"path\":\"{i}\"}}")), KernelLoopVerdict::Proceed);
        }
        let verdict = gov.observe("read_file", "{\"path\":\"7\"}");
        assert!(matches!(verdict, KernelLoopVerdict::Halt { .. }));
    }

    #[test]
    fn loop_governor_soft_limit_at_100_with_repeat_3() {
        let mut gov = KernelLoopGovernor::new();
        // 前 100 次不同工具调用（轮换工具名防同名停滞）：Proceed
        for i in 0..100 {
            let tool = match i % 3 { 0 => "read_file", 1 => "write_file", _ => "list_dir" };
            assert_eq!(gov.observe(tool, &format!("{{\"path\":\"{i}\"}}")), KernelLoopVerdict::Proceed);
        }
        // 第 101 次：turn_calls>100 但 repeat=1，不触发
        assert_eq!(gov.observe("grep_code", "{\"path\":\"new\"}"), KernelLoopVerdict::Proceed);
        // 第 102-103 次：连续 3 次相同 → 触发
        gov.observe("grep_code", "{\"path\":\"new\"}");
        gov.observe("grep_code", "{\"path\":\"new\"}");
        let verdict = gov.observe("grep_code", "{\"path\":\"new\"}");
        assert!(matches!(verdict, KernelLoopVerdict::Halt { .. }));
    }

    #[test]
    fn loop_governor_hard_limit_at_1000() {
        let mut gov = KernelLoopGovernor::new();
        // 1000 次不同工具调用（轮换工具名防同名停滞）：Proceed
        for i in 0..1000 {
            let tool = match i % 3 { 0 => "read_file", 1 => "write_file", _ => "list_dir" };
            assert_eq!(gov.observe(tool, &format!("{{\"path\":\"{i}\"}}")), KernelLoopVerdict::Proceed);
        }
        // 第 1001 次：无条件触发
        let verdict = gov.observe("grep_code", "{\"path\":\"1000\"}");
        assert!(matches!(verdict, KernelLoopVerdict::Halt { .. }));
    }

    #[test]
    fn budget_gate_proceeds_when_under_limit() {
        assert_eq!(
            KernelToolBudgetGate::check(100, 50, 5, 0, 0),
            KernelBudgetVerdict::Proceed
        );
    }

    #[test]
    fn budget_gate_extends_when_recent_successes_and_no_loop_breaks() {
        match KernelToolBudgetGate::check(100, 100, 5, 0, 0) {
            KernelBudgetVerdict::Extend { new_limit } => {
                assert!(new_limit > 100);
            }
            _ => panic!("expected Extend"),
        }
    }

    #[test]
    fn budget_gate_halts_when_loop_breaks_or_low_successes() {
        // loop_breaks > 0 → 不延长
        assert_eq!(
            KernelToolBudgetGate::check(100, 100, 5, 1, 0),
            KernelBudgetVerdict::Halt
        );
        // recent_successes < 3 → 不延长
        assert_eq!(
            KernelToolBudgetGate::check(100, 100, 2, 0, 0),
            KernelBudgetVerdict::Halt
        );
        // extensions >= 2 → 不延长
        assert_eq!(
            KernelToolBudgetGate::check(100, 100, 5, 0, 2),
            KernelBudgetVerdict::Halt
        );
    }

    #[test]
    fn round_router_empty_round_retry_then_stop() {
        let mut router = KernelRoundRouter::new();
        let input = KernelRoundInput {
            text: "",
            has_reasoning: false,
            truncated: false,
            interrupted: false,
            has_native_tool_calls: false,
        };
        // 第 1 次空轮：RetryEmpty
        let actions = router.route(&input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], KernelRoundAction::RetryEmpty { .. }));
        // 第 2 次空轮：StopEmpty
        let actions = router.route(&input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], KernelRoundAction::StopEmpty { .. }));
    }

    #[test]
    fn round_router_proceeds_on_tool_only_response() {
        // 工具调用响应可能无正文（text 空但 has_native_tool_calls=true），应正常 Proceed
        let mut router = KernelRoundRouter::new();
        let input = KernelRoundInput {
            text: "",
            has_reasoning: false,
            truncated: false,
            interrupted: false,
            has_native_tool_calls: true,
        };
        let actions = router.route(&input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], KernelRoundAction::Proceed));
    }

    #[test]
    fn round_router_replay_before_interrupted_continuation() {
        let mut router = KernelRoundRouter::new();
        let input = KernelRoundInput {
            text: "",
            has_reasoning: false,
            truncated: false,
            interrupted: true,
            has_native_tool_calls: false,
        };
        // 前 5 次中断：ReplayFrozen
        for _ in 0..5 {
            let actions = router.route(&input);
            assert_eq!(actions.len(), 1);
            assert!(matches!(actions[0], KernelRoundAction::ReplayFrozen));
        }
        // 第 6 次中断：ContinueInterrupted
        let actions = router.route(&input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], KernelRoundAction::ContinueInterrupted { .. }));
    }

    #[test]
    fn round_router_interrupted_note_falls_through() {
        let mut router = KernelRoundRouter::new();
        // 先用尽中断续写次数（3 次）
        let input = KernelRoundInput {
            text: "partial",
            has_reasoning: false,
            truncated: false,
            interrupted: true,
            has_native_tool_calls: false,
        };
        for _ in 0..3 {
            let actions = router.route(&input);
            assert!(matches!(actions[0], KernelRoundAction::ContinueInterrupted { .. }));
        }
        // 第 4 次中断：InterruptedNote（落穿，无后续动作）
        let actions = router.route(&input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], KernelRoundAction::InterruptedNote { .. }));
    }

    #[test]
    fn round_router_truncated_continuation() {
        let mut router = KernelRoundRouter::new();
        let input = KernelRoundInput {
            text: "partial output",
            has_reasoning: false,
            truncated: true,
            interrupted: false,
            has_native_tool_calls: false,
        };
        // 前 8 次截断：ContinueTruncated
        for _ in 0..8 {
            let actions = router.route(&input);
            assert!(matches!(actions[0], KernelRoundAction::ContinueTruncated { .. }));
        }
        // 第 9 次截断：Proceed（续写次数耗尽，落穿后续 UI 门）
        let actions = router.route(&input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], KernelRoundAction::Proceed));
    }

    #[test]
    fn round_router_fake_call_correction() {
        let mut router = KernelRoundRouter::new();
        let input = KernelRoundInput {
            text: "已调用工具 read_file 读取了文件内容",
            has_reasoning: false,
            truncated: false,
            interrupted: false,
            has_native_tool_calls: false,
        };
        // 前 3 次假调用：CorrectFakeCall
        for _ in 0..3 {
            let actions = router.route(&input);
            assert!(matches!(actions[0], KernelRoundAction::CorrectFakeCall { .. }));
        }
        // 第 4 次假调用：Proceed（纠正次数耗尽）
        let actions = router.route(&input);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], KernelRoundAction::Proceed));
    }

    #[test]
    fn round_router_reasoning_only_continuation() {
        let mut router = KernelRoundRouter::new();
        let input = KernelRoundInput {
            text: "",
            has_reasoning: true,
            truncated: true,
            interrupted: false,
            has_native_tool_calls: false,
        };
        let actions = router.route(&input);
        match &actions[0] {
            KernelRoundAction::ContinueTruncated { reasoning_only, .. } => {
                assert!(reasoning_only);
            }
            _ => panic!("expected ContinueTruncated with reasoning_only=true"),
        }
    }
}
