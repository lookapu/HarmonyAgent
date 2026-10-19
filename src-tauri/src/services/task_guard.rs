//! 任务护栏：防失忆 / 防打转 / 失速检测（进程内状态，按会话隔离，任务开始时重置）
//!
//! 设计目标：
//! - **目标锚定**：记录本轮任务的用户目标，每 N 轮把目标重注入系统提示，防止模型跑题。
//! - **失速检测**：连续多次工具调用没有"实质进展信号"（写文件/构建/部署/测试）时，
//!   触发一次强制建议（构建验证 / 重新规划 / 终止）。
//! - **同文件反复修改**：同一文件被连续编辑达到阈值时，强制要求先构建验证再继续改。
//! - **失败方案黑名单**：记录已失败的命令/动作签名，阻止模型用完全相同的方式重试。
//!
//! 各项阈值默认值定义在 agent_limits，运行时以其动态配置为准（设置页可调，0/-1 表示不限制），
//! 此处常量仅作默认值与测试引用。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 触发"同文件反复编辑后强制构建"的连续编辑次数阈值（默认值；运行时以 agent_limits 配置为准）
pub const REPEAT_EDIT_THRESHOLD: usize = crate::services::agent_limits::DEFAULT_REPEAT_EDIT_THRESHOLD as usize;
/// 触发失速检测的"无进展"工具调用次数（默认值；运行时以 agent_limits 配置为准）
pub const STALL_TOOL_THRESHOLD: usize = crate::services::agent_limits::DEFAULT_STALL_TOOL_THRESHOLD as usize;
/// 目标锚定重注入间隔（每 N 轮工具调用，默认值；运行时以 agent_limits 配置为准）
pub const GOAL_REINJECT_INTERVAL: usize = crate::services::agent_limits::DEFAULT_GOAL_REINJECT_INTERVAL as usize;
/// 构建连续失败达到此次数时强制收敛（默认值；运行时以 agent_limits 配置为准）
pub const BUILD_FAIL_CONVERGE_THRESHOLD: usize = crate::services::agent_limits::DEFAULT_BUILD_FAIL_CONVERGE_THRESHOLD as usize;
/// 失败动作黑名单阈值（默认值；运行时以 agent_limits 配置为准）
pub const BLACKLIST_FAIL_THRESHOLD: usize = crate::services::agent_limits::DEFAULT_BLACKLIST_FAIL_THRESHOLD as usize;

#[derive(Default, Debug)]
struct ConversationGuard {
    /// 本轮任务的用户目标（用于锚定重注入）
    goal: Option<String>,
    /// 目标重注入轮次计数（goal-round：每注入一次 +1，注入文案带轮次）
    goal_rounds: usize,
    /// 工具调用计数（本轮任务内累计）
    tool_count: usize,
    /// 距离上次"实质进展"的工具调用数
    since_progress: usize,
    /// 停滞期**工具结果**累计进上下文的字节数（发生实质进展即清零）
    ///
    /// 为什么要有它：T-8 实测 9 字提问烧掉 209,825 输入 token，而**工具总耗时只占 2.2%**
    /// ——成本几乎全在「每个字节进上下文」。而护栏此前**只数轮次**，
    /// 对这笔钱完全看不见。
    stalled_bytes: usize,
    /// 已通知给用户的失速级别（0 = 从未通知）
    ///
    /// 用来做**「级别变化才通知」**：失速期间每个工具调用都会命中护栏，
    /// 若每次都发事件，前端会被同一条提示刷屏；反过来，
    /// 用户真正需要知道的是「级别变了」和「恢复了」这两个时刻。
    notified_stage: u8,
    /// 文件 → 连续编辑次数（发生其他进展动作时清零）
    edit_counts: HashMap<String, usize>,
    /// 最近一次被编辑的文件（用于连续计数）
    last_edited: Option<String>,
    /// 已失败动作签名（命令/工具+关键参数哈希）→ 失败次数
    failed_actions: HashMap<String, usize>,
    /// 连续构建失败次数（构建成功时清零）
    build_fail_streak: usize,
}

static GUARDS: OnceLock<Mutex<HashMap<String, ConversationGuard>>> = OnceLock::new();

fn guards() -> &'static Mutex<HashMap<String, ConversationGuard>> {
    GUARDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn guard_mut<F: FnOnce(&mut ConversationGuard) -> R, R>(conversation_id: &str, f: F) -> R {
    let mut map = guards().lock().unwrap();
    let g = map.entry(conversation_id.to_string()).or_default();
    f(g)
}

/// 失速级别（0 = 无，1/2/3 = 阶梯）。
///
/// **单一真源**：`stall_message` 的分级与「要不要告诉用户」都必须走这里，
/// 否则两处各算一次就会漂移（文案说第 2 级、通知说第 1 级）。
///
/// - 0：未达阈值（**不通知**——刚到阈值的正常任务不该被惊扰）
/// - 1：刚到阈值
/// - 2：`2 × 阈值`
/// - 3：`3 × 阈值` 及以上（封顶，不再往上升）
///
/// `threshold` 为 `None` 表示该护栏被用户关掉（0 / -1 = 不限制）⇒ 恒为 0。
pub fn stall_stage(since_progress: usize, threshold: Option<usize>) -> u8 {
    let Some(t) = threshold else { return 0 };
    if since_progress < t { return 0; }
    // 阈值可能为 0（纯函数不该假设调用方已过滤），除法至少按 1 算
    let raw = since_progress / t.max(1);
    raw.min(3) as u8
}

/// 失速提示的**阶梯文案**。
///
/// ## 为什么要分级（第二十四批修 T-8）
///
/// 原实现是一段平铺文字，只有轮次数字在变。T-8 实测：**同一句话模型读到 16 遍，
/// 一次都没有改变策略**，继续只读探索到最后一轮——9 字提问烧掉 209,825 输入 token。
/// 也就是说这不是「提醒得不够勤」，而是**重复的同一段文字会被当成背景噪声跳过**。
/// 分级之后每次出现的形态都不同，模型至少必须重新读一遍才能判断。
///
/// ## 阶段按**阈值的倍数**切，不按绝对轮次
///
/// `stall_tool_threshold` 在设置页可调（用户可能设 3，也可能设 30）。
/// 写死绝对轮次会让「阈值=30」的用户永远停在第 1 级、「阈值=3」的用户一步冲到顶。
///
/// ## `stalled_bytes` 是这笔成本唯一的可见数字
///
/// T-8 另一条实测：工具总耗时只占 **2.2%**，97.8% 是模型推理 ⇒
/// **成本几乎全在「每个字节进上下文」**。而预算只按轮次与时长计价，
/// 从不按字节计价 ⇒ 护栏此前对这笔钱完全失明。
///
/// ## 边界：这里**只升级措辞，不强制收尾**
///
/// 「该在哪一轮强制中断」是产品判断（见台账 T-8 的待定部分），**本函数不碰它**：
/// 三级文案最重也只是要求模型「本轮必须二选一」，**不会真的终止循环**。
/// 越过这条线就不是工程判断而是替用户做产品决定了。
fn stall_message(since_progress: usize, threshold: usize, stalled_bytes: usize) -> String {
    let kb = stalled_bytes / 1024;
    match stall_stage(since_progress, Some(threshold)) {
        2 => format!(
            "【失速提醒 · 第 2 级】已连续 {since_progress} 次无实质进展（阈值 {threshold}），\
             本次停滞期间工具结果已累计约 {kb} KB 进上下文。\n\
             **重复的同类只读调用不会再带来新信息**——请在本轮内做一个**可验证**的动作：\
             build_project / run_tests / deploy，或明确写出你接下来要验证的那**一个**假设，\
             然后去验证它。不要再增加同类只读调用。"
        ),
        3 => format!(
            "【失速提醒 · 第 3 级】已连续 {since_progress} 次无实质进展（阈值 {threshold}，\
             累计约 {kb} KB 进上下文）。\n\
             **本轮必须二选一**：① 产出一个可验证的进展（构建 / 测试 / 部署）；\
             ② 向用户汇报当前现状、已排除的可能、以及建议的下一步。\n\
             继续只读探索是**纯成本**——每次这样的调用都在消耗上下文预算却不产生结论。"
        ),
        // 0 与 1 都用「未分级」文案。
        // ⚠️ 0 实际**不可达**（调用方保证 `since_progress >= threshold`），
        //    但纯函数不该在不可达分支上写出「第 3 级」这种与级别自相矛盾的文案——
        //    变异验证正是靠这一点抓出过一个**真隐患**（旧写法把 0 落进第 3 级分支）。
        _ => format!(
            "已连续 {since_progress} 次工具调用未产生实质进展（无写文件/构建/部署/测试）。\
             若仍在排查请继续推进，但建议尽快进入验证环节（有代码改动就 build_project，\
             能部署就 deploy）；不要长时间停留在只读探索上。"
        ),
    }
}

/// 任务开始时重置护栏状态并记录目标
pub fn begin_task(conversation_id: &str, goal: &str) {
    let mut map = guards().lock().unwrap();
    map.insert(
        conversation_id.to_string(),
        ConversationGuard {
            goal: Some(goal.to_string()),
            ..Default::default()
        },
    );
}

/// 会话删除等场景：清理该会话的护栏状态，避免进程内状态单调增长
pub fn clear_task(conversation_id: &str) {
    if let Ok(mut map) = guards().lock() {
        map.remove(conversation_id);
    }
}

/// 记录一次工具调用，返回该调用应附加到模型的护栏提示（可能为空）。
/// `progress` 表示该工具是否构成"实质进展"（写文件/构建/部署/测试成功等）。
/// `result_bytes` 是本次工具结果的**字节数**（用于停滞期上下文成本累计，见
/// [`ConversationGuard::stalled_bytes`]）；不关心字节的调用方传 0。
/// 各项阈值取自 agent_limits 动态配置（0/-1 表示不限制）。
pub fn record_tool(
    conversation_id: &str,
    tool: &str,
    args: &serde_json::Value,
    ok: bool,
    result_bytes: usize,
) -> GuardHint {
    let limits = crate::services::agent_limits::current();
    guard_mut(conversation_id, |g| {
        g.tool_count += 1;
        g.stalled_bytes = g.stalled_bytes.saturating_add(result_bytes);

        // 失败动作黑名单：记录失败签名
        if !ok {
            let sig = action_signature(tool, args);
            let entry = g.failed_actions.entry(sig).or_insert(0);
            *entry += 1;
        }

        // 写文件类：累计同文件连续编辑次数
        let edited_files = edited_paths(tool, args);
        for f in &edited_files {
            *g.edit_counts.entry(f.clone()).or_insert(0) += 1;
            g.last_edited = Some(f.clone());
        }

        // 构建结果连续失败计数（构建成功清零；失败累计）
        if matches!(tool, "build_project") {
            if ok {
                g.build_fail_streak = 0;
            } else {
                g.build_fail_streak += 1;
            }
        }

        let is_progress = is_progress_action(tool, ok, !edited_files.is_empty());
        if is_progress {
            g.since_progress = 0;
            // 停滞期累计的字节同样要清零：它只描述「这一次停滞」花了多少上下文，
            // 发生过进展后这个数字就不再有意义（否则会一直涨成一个无关的总数）。
            g.stalled_bytes = 0;
            // 发生构建/部署/测试等强验证动作后，清空文件连续编辑计数
            if matches!(tool, "build_project" | "deploy" | "run_tests") {
                g.edit_counts.clear();
            }
        } else {
            g.since_progress += 1;
        }

        // 组装提示
        let mut hint = GuardHint::default();
        // 失速级别与「变化」标记：与文案分级**同源**（`stall_stage` 是单一真源），
        // 供调用方决定要不要通知用户。级别与文案分开算的话，
        // 会出现「文案说第 2 级、提示条说第 1 级」这种对不上的状态。
        let stall_threshold = limits.stall_tool_threshold();
        let stage = stall_stage(g.since_progress, stall_threshold);
        hint.stall_stage = stage;
        hint.stall_stage_changed = stage != g.notified_stage;
        hint.stalled_bytes = g.stalled_bytes;
        hint.since_progress = g.since_progress;
        hint.stall_threshold = stall_threshold.unwrap_or(0);
        g.notified_stage = stage;
        if let Some(f) = &g.last_edited {
            if let Some(n) = g.edit_counts.get(f) {
                if let Some(threshold) = limits.repeat_edit_threshold() {
                    if *n >= threshold
                        && !matches!(tool, "build_project" | "deploy" | "run_tests")
                    {
                        hint.force_verify = Some(format!(
                            "文件 {f} 已被连续修改 {n} 次但未经验证。请立即调用 build_project 构建验证（若构建失败，依据错误定位修复），不要继续盲目编辑同一文件。"
                        ));
                    }
                }
            }
        }
        if let Some(threshold) = stall_threshold {
            if g.since_progress >= threshold && hint.force_verify.is_none() {
                hint.stall_warning = Some(stall_message(
                    g.since_progress,
                    threshold,
                    g.stalled_bytes,
                ));
            }
        }
        // 构建连续失败收敛：达到阈值后强制换思路或汇报，避免无限"改→构建失败→再改"循环
        if let Some(threshold) = limits.build_fail_converge_threshold() {
            if g.build_fail_streak >= threshold {
                hint.converge_warning = Some(format!(
                    "已连续 {} 次构建失败。请换一种方式排查：1) 回看最近几次构建错误是否同一根因、当前修复是否真正命中（读完整 build.log 而非只看尾）；2) 检查环境层面（SDK 路径/签名/依赖安装）而非只改代码；3) 若确实无法推进，向用户汇报现状并给出建议。",
                    g.build_fail_streak
                ));
            }
        }
        if let Some(interval) = limits.goal_reinject_interval() {
            if g.tool_count % interval == 0 {
                if let Some(goal) = &g.goal {
                    g.goal_rounds += 1;
                    // 目标轮次注入（对齐 deepseek-harness goal-round-driver 的 <goal_round> 提示）：
                    // 轮次计数 + 证据化完成要求——宣告完成前必须核对证据（构建/测试/文件/截图），
                    // 防“完成声明无验证背书”的假收尾（与收尾复核的 ship 审计互补）
                    hint.goal_reminder = Some(format!(
                        "<goal_round>\n目标：{goal}\n轮次：{}\n继续推进：以当前工作区、工具结果与会话状态为准，不要假设早期叙述仍然有效；每次工具调用都要产出实质进展并验证结果。\n在宣告任务完成之前，必须先核对完成证据（构建输出/测试结果/文件内容/截图等）并在总结中明确引用；若还有未完成或未验证的环节，继续调用工具推进，不要提前收尾。\n</goal_round>",
                        g.goal_rounds
                    ));
                }
            }
        }
        hint
    })
}

/// 读取当前任务锚定目标（收尾复核/完成判定用；任务未开始或无目标时返回 None）
pub fn current_goal(conversation_id: &str) -> Option<String> {
    guard_mut(conversation_id, |g| g.goal.clone())
}

/// 检查某动作签名是否已在本任务失败过（用于阻止完全相同的重试）。
/// 阈值取自 agent_limits 动态配置（0/-1 表示关闭黑名单）。
pub fn is_blacklisted(conversation_id: &str, tool: &str, args: &serde_json::Value) -> bool {
    let Some(threshold) = crate::services::agent_limits::current().blacklist_fail_threshold() else {
        return false;
    };
    guard_mut(conversation_id, |g| {
        let sig = action_signature(tool, args);
        g.failed_actions.get(&sig).copied().unwrap_or(0) >= threshold
    })
}

#[derive(Default, Debug)]
pub struct GuardHint {
    /// 强制构建验证提示
    pub force_verify: Option<String>,
    /// 失速警告
    pub stall_warning: Option<String>,
    /// 目标锚定提醒
    pub goal_reminder: Option<String>,
    /// 连续构建失败收敛警告
    pub converge_warning: Option<String>,
    /// 当前失速级别（0 = 无；1/2/3 = 阶梯）。见 [`stall_stage`]
    pub stall_stage: u8,
    /// 级别**是否刚发生变化**（用于「变化才通知用户」，避免失速期间刷屏）
    pub stall_stage_changed: bool,
    /// 停滞期累计进上下文的字节数
    pub stalled_bytes: usize,
    /// 距离上次实质进展的工具调用数
    pub since_progress: usize,
    /// 当前失速阈值（0 表示该护栏未启用）
    pub stall_threshold: usize,
}

impl GuardHint {
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.force_verify.is_none()
            && self.stall_warning.is_none()
            && self.goal_reminder.is_none()
            && self.converge_warning.is_none()
    }

    /// 拼接为可注入用户消息（工具结果后）的提示文本
    pub fn to_injection(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(m) = &self.goal_reminder {
            parts.push(m.clone());
        }
        if let Some(m) = &self.force_verify {
            parts.push(m.clone());
        }
        if let Some(m) = &self.converge_warning {
            parts.push(m.clone());
        }
        if let Some(m) = &self.stall_warning {
            parts.push(m.clone());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
}

/// 本次调用改动了哪些工作区文件。
///
/// 「哪些工具会改工作区文件」与「路径怎么取」两件事都复用
/// `agent::verification_planner` 的唯一真源，不在本文件另抄一份清单。
/// 原先这里只认 `write_file|edit_file|delete_file` 的 `path` 字段，漏掉
/// `multi_edit`（走 `edits[]`）与 `lsp_rename`（走 `path` 但不在名单里），
/// 后果有两处：
/// 1. `is_progress_action` 把成功改了文件的调用判成「无进展」→ `since_progress`
///    照涨 → 模型正在正确干活，却被注入「你没进展，换个思路」的失速警告；
/// 2. 「同一文件连续编辑 N 次仍未验证」的强制验证提示对这两个工具**永远不触发**，
///    而 `multi_edit` 正是项目自己的错误诊断引导模型使用的编辑工具。
///
/// 返回全部命中的文件而非单个：一次 `multi_edit` 可以改多个文件，
/// 只记第一个会让同文件连续编辑次数被系统性低估。
fn edited_paths(tool: &str, args: &serde_json::Value) -> Vec<String> {
    if !crate::agent::verification_planner::is_mutation_tool(tool) {
        return Vec::new();
    }
    crate::agent::verification_planner::paths_from_args(&args.to_string())
}

fn is_progress_action(tool: &str, ok: bool, edited_any_file: bool) -> bool {
    if !ok {
        return false;
    }
    if edited_any_file {
        return true;
    }
    matches!(tool, "build_project" | "deploy" | "run_tests" | "ohpm_install" | "git_commit")
}

fn action_signature(tool: &str, args: &serde_json::Value) -> String {
    // 对 run_command 用 command 字段做签名；其他工具用工具名 + 排序后的关键参数
    if tool == "run_command" {
        if let Some(c) = args.get("command").and_then(|v| v.as_str()) {
            return format!("run_command:{}", c.trim());
        }
    }
    if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
        return format!("{tool}:{p}");
    }
    tool.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_and_verify() {
        let cid = "test-conv-1";
        begin_task(cid, "做一个记账页");
        // 连续 10 次只读 → 失速
        let v = serde_json::Value::Null;
        for _ in 0..STALL_TOOL_THRESHOLD - 1 {
            let h = record_tool(cid, "list_dir", &v, true, 0);
            assert!(h.stall_warning.is_none());
        }
        let h = record_tool(cid, "list_dir", &v, true, 0);
        assert!(h.stall_warning.is_some());
    }

    /// T-8 的核心断言：**同一段文字不能重复出现**。
    /// 原实现三段文案只差一个数字，模型读到 16 遍一次都没改策略。
    #[test]
    fn stall_warning_escalates_in_three_stages() {
        let t = 10;
        let s1 = stall_message(t, t, 0);
        let s2 = stall_message(t * 2, t, 100 * 1024);
        let s3 = stall_message(t * 3, t, 100 * 1024);
        // 前提：三段确实都存在（否则「两两不等」可能是靠 None 蒙混）
        assert!(!s1.is_empty() && !s2.is_empty() && !s3.is_empty());
        assert_ne!(s1, s2, "第 1、2 级文案相同 ⇒ 模型会把重复文本当噪声跳过");
        assert_ne!(s2, s3, "第 2、3 级文案相同 ⇒ 最高级失去意义");
        assert_ne!(s1, s3, "第 1、3 级文案相同");
        // 第 1 级保持原有措辞（不惊扰刚到阈值的正常任务），2/3 级带阶梯标记
        assert!(!s1.contains("第 2 级") && !s1.contains("第 3 级"));
        assert!(s2.contains("第 2 级"));
        assert!(s3.contains("第 3 级"));
    }

    /// 阶段按**阈值的倍数**切，不按绝对轮次。
    /// 阈值在设置页可调（可能 3 也可能 30），写死绝对值会让其中一端永远停在第 1 级。
    #[test]
    fn stall_stage_follows_the_configured_threshold() {
        for t in [3usize, 10, 30] {
            assert!(!stall_message(t, t, 0).contains("第 2 级"), "阈值 {t}");
            assert!(stall_message(t * 2, t, 0).contains("第 2 级"), "阈值 {t}");
            assert!(stall_message(t * 3, t, 0).contains("第 3 级"), "阈值 {t}");
        }
        // 阈值为 0 时不能除零 panic（agent_limits 允许 0=不限制，但纯函数不该假设）
        assert!(!stall_message(5, 0, 0).is_empty());
    }

    /// 停滞期字节累计：**有实质进展即清零**。
    ///
    /// 写法上刻意**不**只断言「进展后不再失速」——那条只由 `since_progress` 决定，
    /// 变异把 `stalled_bytes = 0` 删掉时它**照样全绿**（这正是本条第一版的缺陷，
    /// 由变异验证抓出来）。真正的判据是：**第二段停滞重新累计到 160KB 而不是 320KB**。
    #[test]
    fn stalled_bytes_reset_on_progress() {
        let cid = "test-conv-bytes";
        begin_task(cid, "查一个问题");
        let v = serde_json::Value::Null;
        let eight_kb = 8 * 1024;

        // 第一段停滞：20 次只读 × 8KB = 160KB → 第 2 级，且文案里应写出这个数
        let mut w1 = String::new();
        for _ in 0..STALL_TOOL_THRESHOLD * 2 {
            if let Some(w) = record_tool(cid, "list_dir", &v, true, eight_kb).stall_warning {
                w1 = w;
            }
        }
        assert!(w1.contains("第 2 级"), "应升到第 2 级：{w1}");
        assert!(w1.contains("160"), "第一段累计应为 160KB：{w1}");

        // 一次实质进展 → since_progress 与 stalled_bytes 同时清零
        let after = record_tool(cid, "build_project", &serde_json::Value::Null, true, eight_kb);
        assert!(after.stall_warning.is_none(), "刚发生过进展不该立刻再失速：{:?}", after.stall_warning);

        // 第二段停滞：同样 20 次 × 8KB。若**没有**清零，这里会显示 320KB。
        let mut w2 = String::new();
        for _ in 0..STALL_TOOL_THRESHOLD * 2 {
            if let Some(w) = record_tool(cid, "list_dir", &v, true, eight_kb).stall_warning {
                w2 = w;
            }
        }
        assert!(w2.contains("第 2 级"), "{w2}");
        assert!(
            w2.contains("160"),
            "进展后应重新从 160KB 起算，而不是累计到 320KB：{w2}"
        );
    }

    /// 失速级别函数本身：0/1/2/3，封顶，阈值关闭时恒 0。
    #[test]
    fn stall_stage_levels() {
        let t = 10usize;
        assert_eq!(stall_stage(0, Some(t)), 0, "未达阈值不应失速");
        assert_eq!(stall_stage(t - 1, Some(t)), 0, "差一次也不该失速");
        assert_eq!(stall_stage(t, Some(t)), 1, "刚到阈值为第 1 级");
        assert_eq!(stall_stage(t * 2, Some(t)), 2, "2 倍阈值为第 2 级");
        assert_eq!(stall_stage(t * 3, Some(t)), 3, "3 倍阈值为第 3 级");
        assert_eq!(stall_stage(t * 9, Some(t)), 3, "再往上封顶在第 3 级");
        assert_eq!(stall_stage(100, None), 0, "护栏被关掉时恒为 0");
        assert_eq!(stall_stage(100, Some(0)), 3, "阈值为 0 不该除零 panic");
    }

    /// 级别与文案**同源**：文案里出现的级别必须与 `stall_stage` 算出来的一致。
    ///
    /// 分两处算的话会出现「提示条说第 1 级、模型看到的是第 2 级」这种对不上的状态，
    /// 而两边各自都「看起来对」。
    ///
    /// ⚠️ 采样必须**跨过分级边界**（`2t-1` / `2t+1` 这类），
    /// 只在 `t` / `2t` / `3t` 这些**整数倍**上采样的话，
    /// 「+1 偏移」那类改法算出来的级别完全相同 ⇒ 判据形同虚设（变异 M4 实测）。
    #[test]
    fn stall_stage_matches_message_level() {
        let t = 10usize;
        for since in [0, 1, t - 1, t, t + 1, t * 2 - 1, t * 2, t * 2 + 1, t * 3 - 1, t * 3, t * 4, t * 9] {
            let level = stall_stage(since, Some(t));
            let msg = stall_message(since, t, 0);
            // 文案里的级别标记：带「第 N 级」的是 2/3，不带的是「未分级」（0 或 1）
            let msg_level = if msg.contains("第 3 级") {
                3
            } else if msg.contains("第 2 级") {
                2
            } else {
                1
            };
            // 级别 0（未达阈值）用「未分级」文案，与级别 1 同形
            let expect = if level == 0 { 1 } else { level };
            assert_eq!(
                msg_level, expect,
                "since={since}（级别 {level}）文案级别与 stall_stage 不一致：{msg}"
            );
        }
    }

    /// **「级别变化才通知」**——失速期间每个工具调用都会命中护栏，
    /// 每次都发事件会把界面刷屏；用户需要知道的是「变严重了」与「恢复了」两个时刻。
    ///
    /// 判据是「整段停滞里恰好通知一次」：删掉 `notified_stage` 记账会变成每次都通知，
    /// 而永远不通知则第一次就丢——两种改法都被这一条挡住。
    #[test]
    fn stall_level_notified_only_on_change() {
        let cid = "test-conv-notify";
        begin_task(cid, "查一个问题");
        let v = serde_json::Value::Null;
        let t = STALL_TOOL_THRESHOLD;

        // 未达阈值：级别恒 0，且**不应**有任何通知
        for _ in 0..t - 1 {
            let h = record_tool(cid, "list_dir", &v, true, 0);
            assert_eq!(h.stall_stage, 0);
            assert!(!h.stall_stage_changed, "级别没变就不该通知");
        }

        // 刚到阈值：首次进入失速 ⇒ 通知一次
        let h = record_tool(cid, "list_dir", &v, true, 0);
        assert_eq!(h.stall_stage, 1, "刚到阈值为第 1 级");
        assert!(h.stall_stage_changed, "首次进入失速必须通知");
        assert_eq!(h.since_progress, t);
        assert_eq!(h.stall_threshold, t);

        // 继续只读直到 2 倍阈值：整段里**只应再通知一次**（升到第 2 级）
        let mut changes = 0;
        let mut stages = Vec::new();
        for _ in 0..t {
            let h = record_tool(cid, "list_dir", &v, true, 1024);
            stages.push(h.stall_stage);
            if h.stall_stage_changed {
                changes += 1;
            }
        }
        assert_eq!(changes, 1, "1→2 级只该通知一次，实得 {changes} 次");
        assert_eq!(stages.last().copied(), Some(2), "末次应升到第 2 级：{stages:?}");

        // 一次实质进展 ⇒ 级别归 0 ⇒ **恢复通知**（这是清空提示条的信号）
        let h = record_tool(cid, "build_project", &serde_json::Value::Null, true, 0);
        assert_eq!(h.stall_stage, 0, "有实质进展后不该还失速");
        assert!(h.stall_stage_changed, "恢复必须通知，否则提示条清不掉");

        // 恢复后**再次**失速：必须重新通知一次。
        //
        // ⚠️ 判据不能只到「恢复会通知」为止：若 `notified_stage` 在恢复时**不清零**，
        //    下一轮从第 1 级重新失速时与旧值相同 ⇒ 判定为「没变化」⇒ **再也不通知**。
        //    用户就看不到第二次失速了——而这正是变异 M5 抓出来的漏网。
        let mut changes = 0;
        for _ in 0..t {
            let h = record_tool(cid, "list_dir", &v, true, 0);
            if h.stall_stage_changed {
                changes += 1;
            }
        }
        assert_eq!(changes, 1, "恢复后再次失速必须重新通知一次，实得 {changes} 次");
    }

    /// 停滞字节要随事件一起带出去（用户据此判断「这轮只读探索花了多少上下文」）。
    #[test]
    fn hint_carries_stalled_bytes() {
        let cid = "test-conv-hint-bytes";
        begin_task(cid, "查一个问题");
        let v = serde_json::Value::Null;
        let eight_kb = 8 * 1024;
        let mut last = 0usize;
        for _ in 0..STALL_TOOL_THRESHOLD {
            last = record_tool(cid, "list_dir", &v, true, eight_kb).stalled_bytes;
        }
        assert_eq!(last, STALL_TOOL_THRESHOLD * eight_kb);
    }

    #[test]
    fn repeat_edit_forces_verify() {
        let cid = "test-conv-2";
        begin_task(cid, "改首页");
        let args = serde_json::json!({"path":"src/Index.ets"});
        for _ in 0..2 {
            let h = record_tool(cid, "edit_file", &args, true, 0);
            assert!(h.force_verify.is_none());
        }
        let h = record_tool(cid, "edit_file", &args, true, 0);
        assert!(h.force_verify.is_some());
        // 构建后清零
        let h = record_tool(cid, "build_project", &serde_json::Value::Null, true, 0);
        assert!(h.force_verify.is_none());
    }

    #[test]
    fn failure_blacklist() {
        let cid = "test-conv-3";
        begin_task(cid, "跑测试");
        let args = serde_json::json!({"command":"npm run test -- --watchAll=false"});
        for _ in 0..BLACKLIST_FAIL_THRESHOLD - 1 {
            record_tool(cid, "run_command", &args, false, 0);
            assert!(!is_blacklisted(cid, "run_command", &args));
        }
        record_tool(cid, "run_command", &args, false, 0);
        assert!(is_blacklisted(cid, "run_command", &args));
    }

    #[test]
    fn build_fail_converges() {
        let cid = "test-conv-build";
        begin_task(cid, "修复构建错误");
        let args = serde_json::Value::Null;
        // 前四次构建失败：尚未触发收敛
        for _ in 0..BUILD_FAIL_CONVERGE_THRESHOLD - 1 {
            let h = record_tool(cid, "build_project", &args, false, 0);
            assert!(h.converge_warning.is_none());
        }
        // 连续第五次失败：触发收敛警告
        let h = record_tool(cid, "build_project", &args, false, 0);
        assert!(h.converge_warning.is_some());
        // 构建成功后清零，下一次失败不再收敛
        record_tool(cid, "build_project", &args, true, 0);
        let h = record_tool(cid, "build_project", &args, false, 0);
        assert!(h.converge_warning.is_none());
    }
}
