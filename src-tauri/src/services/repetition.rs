//! 输出退化（复读）检测：模型在一条回复里把同一段话反复重写时，尽早掐断。
//!
//! 背景（本机实际发生）：一轮回复 16,790 字里，同一段 90 字模板出现了 110 次，占 87.4%，
//! 该轮实际一个工具都没调用，模型只是在正文里反复"承诺"下一步要读哪两段代码。用户只能
//! 手动点停止。现有护栏都拦不住这种情况——它们全是**工具级**判据：
//!   - `agent/kernel_loop.rs` 的循环治理看「同工具+同参数重复调用」；
//!   - `services/task_guard.rs` 的失速检测看「连续 N 次工具调用没有写入/构建」；
//!   - 假调用/行动承诺纠正在**轮末**才触发。
//!
//! 这一轮里没有任何工具调用，于是三道护栏全部落空。本模块补上"文本级"这一道。
//!
//! 判据刻意保守（宁可漏判不可误判）：只看尾部窗口，且要求同一个块**连续**重复至少
//! `MIN_REPEATS` 次，块本身不少于 `MIN_BLOCK` 字节。正常的列表/日志/代码不会把同一段
//! 60+ 字节的文本连着写 6 遍。

/// 重复块最小字节数：太短的周期（如重复的空格、分隔线、单个词）不算退化
const MIN_BLOCK: usize = 60;
/// 重复块最大字节数：超过这个长度的周期性更可能是巧合而非复读
const MAX_BLOCK: usize = 512;
/// 判定为复读所需的最少连续重复次数
const MIN_REPEATS: usize = 6;
/// 尾部检测窗口字节数（只看尾部，避免长文本上做全量比较）
const WINDOW: usize = 4096;
/// 每增长这么多字节才做一次检测（控制开销，避免逐 delta 全窗扫描）
const CHECK_EVERY: usize = 512;

/// 复读判定的增量守卫：随流式文本增长调用 `observe`。
pub struct RepeatGuard {
    /// 下次检测的文本长度门槛（字节）
    next_check_at: usize,
    /// 已判定的复读块周期（字节）
    period: usize,
    /// 复读起点（字节索引，指向重复块的**首次**出现处）
    cut_at: usize,
}

impl Default for RepeatGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl RepeatGuard {
    pub fn new() -> Self {
        Self {
            next_check_at: CHECK_EVERY,
            period: 0,
            cut_at: 0,
        }
    }

    /// 观察累积文本；返回 true 表示判定为复读（此后应停止本轮生成）。
    pub fn observe(&mut self, text: &str) -> bool {
        if self.period > 0 {
            return true;
        }
        if text.len() < self.next_check_at {
            return false;
        }
        self.next_check_at = text.len() + CHECK_EVERY;
        let Some((period, keep)) = repeated_tail(text) else {
            return false;
        };
        self.period = period;
        // 只保留重复块的首次出现，丢掉后面的复读
        self.cut_at = keep;
        true
    }

    /// 是否已判定复读
    pub fn degenerated(&self) -> bool {
        self.period > 0
    }

    /// 裁掉复读尾巴后的文本（未判定复读时原样返回）
    pub fn trim<'a>(&self, text: &'a str) -> &'a str {
        if self.period == 0 || self.cut_at == 0 || self.cut_at >= text.len() {
            return text;
        }
        let mut at = self.cut_at;
        while at < text.len() && !text.is_char_boundary(at) {
            at += 1;
        }
        &text[..at]
    }
}

/// 尾部复读检测：返回 `(周期字节数, 应保留的字节数)`。
///
/// 分两步，兼顾开销与准确：
/// 1. 只在**尾部窗口**上求最小周期（前缀函数，O(W)）——长文本上不必全量求周期；
/// 2. 拿这个候选周期在**全文**上回扫，确认尾部真的在连排同一个块。窗口内的周期可能只是
///    巧合（无关内容凑出的周期），全文回扫会把这种立即否掉；同时回扫给出精确的裁切点，
///    否则裁切点会被窗口左边界卡住，前面残留的复读去不掉。
pub fn repeated_tail(text: &str) -> Option<(usize, usize)> {
    if text.len() < MIN_BLOCK * MIN_REPEATS {
        return None;
    }
    let bytes = text.as_bytes();
    let mut w_start = text.len().saturating_sub(WINDOW);
    while w_start < text.len() && !text.is_char_boundary(w_start) {
        w_start += 1;
    }
    let period = minimal_period(&bytes[w_start..]);
    if !(MIN_BLOCK..=MAX_BLOCK).contains(&period) {
        return None;
    }
    // 全文回扫周期性区段：i 停在周期区的起点
    let mut i = bytes.len();
    while i > period && bytes[i - 1] == bytes[i - 1 - period] {
        i -= 1;
    }
    let run = bytes.len() - i;
    // +1：周期区起点处那个块本身也是一次出现
    if run / period + 1 < MIN_REPEATS {
        return None;
    }
    // i 落在"第二个块"的起点，故 i 之前（含 i 处那一个块）就是首次出现
    Some((period, i))
}

/// 字符串（字节切片）的最小周期，由前缀函数求得；无周期时返回长度本身。
fn minimal_period(w: &[u8]) -> usize {
    if w.is_empty() {
        return 0;
    }
    let mut pi = vec![0usize; w.len()];
    for i in 1..w.len() {
        let mut j = pi[i - 1];
        while j > 0 && w[i] != w[j] {
            j = pi[j - 1];
        }
        if w[i] == w[j] {
            j += 1;
        }
        pi[i] = j;
    }
    w.len() - pi[w.len() - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 事故现场原文的一段（按库里的真实内容）：同一段连续重复
    const LOOP_BLOCK: &str = "按 explore 阶段规则继续推进。每次发起不同的 start/lines 组合。本轮 2 个独立只读请求：\n\
- `Breakpoint.ets` start=1, lines=40\n\
- `FeihuaGamePage.ets` start=614, lines=36\n\n";

    #[test]
    fn detects_the_real_incident_repetition() {
        let prefix = "收到 explore 阶段工具集切换 + 用户门禁。先补两段证据。\n\n";
        let text = format!("{prefix}{}", LOOP_BLOCK.repeat(40));
        let (period, keep) = repeated_tail(&text).expect("应判定为复读");
        assert_eq!(period, LOOP_BLOCK.len());
        // 复读前的正文完整保留；重复块只留一次（裁切点可以落在块内的空白上，不必对齐到块尾）
        let kept = &text[..keep];
        assert!(kept.starts_with(prefix), "复读前的正文不能被裁掉");
        assert!(kept.len() < prefix.len() + 2 * period, "只应保留一次重复块：{}", kept.len());
        assert_eq!(
            kept.matches("- `Breakpoint.ets` start=1, lines=40").count(),
            1,
            "重复内容应只留一份"
        );
    }

    #[test]
    fn guard_stops_and_trims_on_the_repeat() {
        let mut guard = RepeatGuard::new();
        let mut text = String::from("先看枚举定义：\n");
        text.push_str("export enum BreakpointType { xs, sm, md, lg, xl }\n");
        // 边增长边观察：模拟流式增量
        for _ in 0..40 {
            text.push_str(LOOP_BLOCK);
            if guard.observe(&text) {
                break;
            }
        }
        assert!(guard.degenerated(), "连续重复应被判定");
        let trimmed = guard.trim(&text);
        // 只留一份重复块，且保留复读前的正文
        assert!(trimmed.starts_with("先看枚举定义："));
        assert_eq!(trimmed.matches("- `Breakpoint.ets` start=1").count(), 1, "复读尾巴应被裁掉：{}", trimmed.len());
    }

    #[test]
    fn normal_long_prose_is_not_flagged() {
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!(
                "第 {i} 步：检查模块 {i} 的导出符号，确认调用方是否只依赖 BreakpointConstants；如果有其他依赖，记录下来。\n"
            ));
        }
        assert!(repeated_tail(&text).is_none(), "逐句不同的长文不该被判复读");
        let mut guard = RepeatGuard::new();
        assert!(!guard.observe(&text));
    }

    #[test]
    fn single_occurrence_is_not_a_repeat() {
        let text = format!("{}{}", "正文开头。\n".repeat(20), LOOP_BLOCK);
        assert!(repeated_tail(&text).is_none(), "只出现一次不算复读");
    }

    #[test]
    fn tiny_period_is_ignored() {
        // 分隔线/空行反复：周期远小于 MIN_BLOCK，不该判退化
        let text = "----------\n".repeat(300);
        assert!(repeated_tail(&text).is_none());
    }

    #[test]
    fn five_repeats_is_below_threshold() {
        let text = LOOP_BLOCK.repeat(5);
        assert!(repeated_tail(&text).is_none(), "5 次低于阈值，保守放行");
        assert!(repeated_tail(&LOOP_BLOCK.repeat(6)).is_some(), "6 次应命中");
    }

    #[test]
    fn minimal_period_matches_known_strings() {
        assert_eq!(minimal_period(b"abcabcabc"), 3);
        assert_eq!(minimal_period(b"abcd"), 4);
        assert_eq!(minimal_period(b"aaaa"), 1);
        assert_eq!(minimal_period(b""), 0);
    }

    #[test]
    fn trim_keeps_text_intact_when_not_degenerated() {
        let guard = RepeatGuard::new();
        let text = "一切正常的一段正文";
        assert_eq!(guard.trim(text), text);
    }
}
