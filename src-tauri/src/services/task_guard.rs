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
    /// 工具调用计数（本轮任务内累计）
    tool_count: usize,
    /// 距离上次"实质进展"的工具调用数
    since_progress: usize,
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
/// 各项阈值取自 agent_limits 动态配置（0/-1 表示不限制）。
pub fn record_tool(
    conversation_id: &str,
    tool: &str,
    args: &serde_json::Value,
    ok: bool,
) -> GuardHint {
    let limits = crate::services::agent_limits::current();
    guard_mut(conversation_id, |g| {
        g.tool_count += 1;

        // 失败动作黑名单：记录失败签名
        if !ok {
            let sig = action_signature(tool, args);
            let entry = g.failed_actions.entry(sig).or_insert(0);
            *entry += 1;
        }

        // 写文件类：累计同文件连续编辑次数
        let edited_file = edited_path(tool, args);
        if let Some(f) = &edited_file {
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

        let is_progress = is_progress_action(tool, ok, &edited_file);
        if is_progress {
            g.since_progress = 0;
            // 发生构建/部署/测试等强验证动作后，清空文件连续编辑计数
            if matches!(tool, "build_project" | "deploy" | "run_tests") {
                g.edit_counts.clear();
            }
        } else {
            g.since_progress += 1;
        }

        // 组装提示
        let mut hint = GuardHint::default();
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
        if let Some(threshold) = limits.stall_tool_threshold() {
            if g.since_progress >= threshold && hint.force_verify.is_none() {
                hint.stall_warning = Some(format!(
                    "已连续 {} 次工具调用未产生实质进展（无写文件/构建/部署/测试）。若仍在排查请继续推进，但建议尽快进入验证环节（有代码改动就 build_project，能部署就 deploy）；不要长时间停留在只读探索上。",
                    g.since_progress
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
                    hint.goal_reminder = Some(format!(
                        "【目标锚定】用户的原始任务是：{goal}\n请确认当前工具调用仍在服务该目标；若已偏离，回到目标或向用户确认。"
                    ));
                }
            }
        }
        hint
    })
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

fn edited_path(tool: &str, args: &serde_json::Value) -> Option<String> {
    match tool {
        "write_file" | "edit_file" | "delete_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

fn is_progress_action(tool: &str, ok: bool, edited_file: &Option<String>) -> bool {
    if !ok {
        return false;
    }
    if edited_file.is_some() {
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
            let h = record_tool(cid, "list_dir", &v, true);
            assert!(h.stall_warning.is_none());
        }
        let h = record_tool(cid, "list_dir", &v, true);
        assert!(h.stall_warning.is_some());
    }

    #[test]
    fn repeat_edit_forces_verify() {
        let cid = "test-conv-2";
        begin_task(cid, "改首页");
        let args = serde_json::json!({"path":"src/Index.ets"});
        for _ in 0..2 {
            let h = record_tool(cid, "edit_file", &args, true);
            assert!(h.force_verify.is_none());
        }
        let h = record_tool(cid, "edit_file", &args, true);
        assert!(h.force_verify.is_some());
        // 构建后清零
        let h = record_tool(cid, "build_project", &serde_json::Value::Null, true);
        assert!(h.force_verify.is_none());
    }

    #[test]
    fn failure_blacklist() {
        let cid = "test-conv-3";
        begin_task(cid, "跑测试");
        let args = serde_json::json!({"command":"npm run test -- --watchAll=false"});
        for _ in 0..BLACKLIST_FAIL_THRESHOLD - 1 {
            record_tool(cid, "run_command", &args, false);
            assert!(!is_blacklisted(cid, "run_command", &args));
        }
        record_tool(cid, "run_command", &args, false);
        assert!(is_blacklisted(cid, "run_command", &args));
    }

    #[test]
    fn build_fail_converges() {
        let cid = "test-conv-build";
        begin_task(cid, "修复构建错误");
        let args = serde_json::Value::Null;
        // 前四次构建失败：尚未触发收敛
        for _ in 0..BUILD_FAIL_CONVERGE_THRESHOLD - 1 {
            let h = record_tool(cid, "build_project", &args, false);
            assert!(h.converge_warning.is_none());
        }
        // 连续第五次失败：触发收敛警告
        let h = record_tool(cid, "build_project", &args, false);
        assert!(h.converge_warning.is_some());
        // 构建成功后清零，下一次失败不再收敛
        record_tool(cid, "build_project", &args, true);
        let h = record_tool(cid, "build_project", &args, false);
        assert!(h.converge_warning.is_none());
    }
}
