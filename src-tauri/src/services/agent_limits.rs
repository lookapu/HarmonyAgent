//! Agent 护栏参数动态配置（设置页可调，0/-1 = 不限制）。
//!
//! 此前 tool_limits / task_guard / chat 的护栏阈值全是编译期常量，调整需改代码重编译。
//! 本模块把全部护栏参数统一为一份可持久化配置：
//! - 持久化：settings 表 key = "agent_limits"（JSON）；
//! - 生效：进程内全局缓存（RwLock），启动时惰性从 DB 加载，设置页保存时同步更新；
//! - 语义：字段值为 0 或 -1 表示关闭该项护栏（不限制），其余正整数按原值生效；
//!   超过 MAX_LIMIT_VALUE 或小于 -1 的值视为非法（防御误填炸掉任务）。
//!
//! 热路径（每轮工具调用的预算检查）只读全局读锁，开销可忽略，不触碰数据库。

use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};

/// settings 表存储键
pub const SETTINGS_KEY: &str = "agent_limits";

// ---------- 默认值（与历史硬编码常量保持一致，作为无配置时的兜底） ----------

/// 单任务工具调用总预算（兜底护栏：正常任务几十上百次调用足够，仅防极端失控烧 token）
pub const DEFAULT_TOOL_CALL_LIMIT: i64 = 300;
/// 重操作（build/deploy/run_command）单任务最多调用次数
pub const DEFAULT_HEAVY_TOOL_CALL_LIMIT: i64 = 6;
/// 打转检测阈值：同一 (工具, 参数) 连续重复 N 次判定打转
pub const DEFAULT_REPEAT_CALL_LIMIT: i64 = 5;
/// 主 Agent 工具轮次上限（一轮可含多个工具标记）
pub const DEFAULT_TOOL_ROUNDS: i64 = 80;
/// 单任务最长执行时长（分钟），超时优雅停止并保存部分内容
pub const DEFAULT_TASK_DURATION_MINUTES: i64 = 30;
/// 子 Agent 内部循环轮次上限
pub const DEFAULT_SUB_AGENT_ROUNDS: i64 = 20;
/// 失败动作黑名单阈值：同一签名失败达到此次数才拦截
pub const DEFAULT_BLACKLIST_FAIL_THRESHOLD: i64 = 4;
/// 同文件连续编辑后强制构建验证的阈值
pub const DEFAULT_REPEAT_EDIT_THRESHOLD: i64 = 3;
/// 失速检测阈值：连续无实质进展的工具调用次数
pub const DEFAULT_STALL_TOOL_THRESHOLD: i64 = 10;
/// 构建连续失败达到此次数时强制收敛（停止盲目重试）
pub const DEFAULT_BUILD_FAIL_CONVERGE_THRESHOLD: i64 = 5;
/// 目标锚定重注入间隔（每 N 轮工具调用把用户目标重注入系统提示）
pub const DEFAULT_GOAL_REINJECT_INTERVAL: i64 = 8;

/// 配置项合法范围上限（防御异常大值）
const MAX_LIMIT_VALUE: i64 = 1_000_000;

/// Agent 护栏参数。字段为 0 或 -1 表示不限制（关闭该项护栏）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentLimits {
    /// 单任务工具调用总预算
    pub tool_call_limit: i64,
    /// 重操作（build/deploy/run_command）单任务最多调用次数
    pub heavy_tool_call_limit: i64,
    /// 打转检测阈值：同一 (工具, 参数) 连续重复 N 次判定打转
    pub repeat_call_limit: i64,
    /// 主 Agent 工具轮次上限
    pub tool_rounds: i64,
    /// 单任务最长执行时长（分钟）
    pub task_duration_minutes: i64,
    /// 子 Agent 内部循环轮次上限
    pub sub_agent_rounds: i64,
    /// 失败动作黑名单阈值
    pub blacklist_fail_threshold: i64,
    /// 同文件连续编辑后强制构建验证的阈值
    pub repeat_edit_threshold: i64,
    /// 失速检测阈值（无实质进展的工具调用次数）
    pub stall_tool_threshold: i64,
    /// 构建连续失败收敛阈值
    pub build_fail_converge_threshold: i64,
    /// 目标锚定重注入间隔（每 N 轮工具调用）
    pub goal_reinject_interval: i64,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            tool_call_limit: DEFAULT_TOOL_CALL_LIMIT,
            heavy_tool_call_limit: DEFAULT_HEAVY_TOOL_CALL_LIMIT,
            repeat_call_limit: DEFAULT_REPEAT_CALL_LIMIT,
            tool_rounds: DEFAULT_TOOL_ROUNDS,
            task_duration_minutes: DEFAULT_TASK_DURATION_MINUTES,
            sub_agent_rounds: DEFAULT_SUB_AGENT_ROUNDS,
            blacklist_fail_threshold: DEFAULT_BLACKLIST_FAIL_THRESHOLD,
            repeat_edit_threshold: DEFAULT_REPEAT_EDIT_THRESHOLD,
            stall_tool_threshold: DEFAULT_STALL_TOOL_THRESHOLD,
            build_fail_converge_threshold: DEFAULT_BUILD_FAIL_CONVERGE_THRESHOLD,
            goal_reinject_interval: DEFAULT_GOAL_REINJECT_INTERVAL,
        }
    }
}

impl AgentLimits {
    /// 校验并归一化：0 与 -1 统一为 -1（不限制）；非法值返回 Err。
    /// 前端提交任意整数字段，保存前必须过此关。
    pub fn normalize(&mut self) -> Result<(), String> {
        let fields = [
            &mut self.tool_call_limit,
            &mut self.heavy_tool_call_limit,
            &mut self.repeat_call_limit,
            &mut self.tool_rounds,
            &mut self.task_duration_minutes,
            &mut self.sub_agent_rounds,
            &mut self.blacklist_fail_threshold,
            &mut self.repeat_edit_threshold,
            &mut self.stall_tool_threshold,
            &mut self.build_fail_converge_threshold,
            &mut self.goal_reinject_interval,
        ];
        for v in fields {
            if *v == 0 {
                *v = -1; // 0 与 -1 同义：不限制
            } else if *v < -1 || *v > MAX_LIMIT_VALUE {
                return Err(format!(
                    "非法限制值 {v}：请填写 -1、0（均表示不限制）或 1~{MAX_LIMIT_VALUE} 之间的正整数"
                ));
            }
        }
        Ok(())
    }

    fn opt(v: i64) -> Option<usize> {
        if v <= 0 { None } else { Some(v as usize) }
    }

    /// 单任务工具调用总预算（None = 不限制）
    pub fn tool_calls(&self) -> Option<usize> {
        Self::opt(self.tool_call_limit)
    }
    /// 重操作单任务最多调用次数（None = 不限制）
    pub fn heavy_tool_calls(&self) -> Option<usize> {
        Self::opt(self.heavy_tool_call_limit)
    }
    /// 打转检测阈值（None = 关闭打转检测）
    pub fn repeat_calls(&self) -> Option<usize> {
        Self::opt(self.repeat_call_limit)
    }
    /// 主 Agent 工具轮次上限（None = 不限制）
    pub fn tool_rounds(&self) -> Option<usize> {
        Self::opt(self.tool_rounds)
    }
    /// 单任务最长执行时长秒数（None = 不限制）
    pub fn task_duration_secs(&self) -> Option<u64> {
        if self.task_duration_minutes <= 0 {
            None
        } else {
            Some(self.task_duration_minutes as u64 * 60)
        }
    }
    /// 子 Agent 内部循环轮次上限（None = 不限制）
    pub fn sub_agent_rounds(&self) -> Option<usize> {
        Self::opt(self.sub_agent_rounds)
    }
    /// 失败动作黑名单阈值（None = 关闭黑名单）
    pub fn blacklist_fail_threshold(&self) -> Option<usize> {
        Self::opt(self.blacklist_fail_threshold)
    }
    /// 同文件连续编辑后强制构建验证阈值（None = 关闭）
    pub fn repeat_edit_threshold(&self) -> Option<usize> {
        Self::opt(self.repeat_edit_threshold)
    }
    /// 失速检测阈值（None = 关闭失速检测）
    pub fn stall_tool_threshold(&self) -> Option<usize> {
        Self::opt(self.stall_tool_threshold)
    }
    /// 构建连续失败收敛阈值（None = 关闭收敛提示）
    pub fn build_fail_converge_threshold(&self) -> Option<usize> {
        Self::opt(self.build_fail_converge_threshold)
    }
    /// 目标锚定重注入间隔（None = 不重注入）
    pub fn goal_reinject_interval(&self) -> Option<usize> {
        Self::opt(self.goal_reinject_interval)
    }
}

/// 进程内全局配置缓存：启动时从 DB 惰性加载，保存时同步更新。
/// 热路径读锁开销可忽略；DB 不可用（如 full_fetch 工具进程）时保持默认值。
static CURRENT: OnceLock<RwLock<AgentLimits>> = OnceLock::new();

fn current_lock() -> &'static RwLock<AgentLimits> {
    CURRENT.get_or_init(|| RwLock::new(AgentLimits::default()))
}

/// 当前生效的护栏配置（拷贝，字段量小）
pub fn current() -> AgentLimits {
    current_lock().read().map(|g| g.clone()).unwrap_or_default()
}

/// 从 settings 表加载配置并更新全局缓存（启动时调用一次；失败静默保留默认值）。
/// 使用 db::global() 全局连接，不依赖 tauri State，任何模块均可安全调用。
pub fn init_from_db() {
    let Some(db) = crate::db::global() else { return };
    let Ok(conn) = db.lock() else { return };
    let Some(raw) = crate::db::queries::get_setting(&conn, SETTINGS_KEY).ok().flatten() else {
        return;
    };
    let Ok(mut limits) = serde_json::from_str::<AgentLimits>(&raw) else {
        return;
    };
    if limits.normalize().is_ok() {
        if let Ok(mut g) = current_lock().write() {
            *g = limits;
        }
    }
}

/// 保存配置：校验归一化 → 写 settings 表 → 更新全局缓存。
pub fn save(limits: &AgentLimits) -> Result<AgentLimits, String> {
    let mut normalized = limits.clone();
    normalized.normalize()?;
    let db = crate::db::global().ok_or("数据库不可用，无法保存限制配置")?;
    let conn = db.lock().map_err(|e| e.to_string())?;
    let raw = serde_json::to_string(&normalized).map_err(|e| e.to_string())?;
    crate::db::queries::set_setting(&conn, SETTINGS_KEY, &raw).map_err(|e| e.to_string())?;
    drop(conn);
    if let Ok(mut g) = current_lock().write() {
        *g = normalized.clone();
    }
    Ok(normalized)
}

/// 恢复默认配置（等价于用默认值执行一次 save）
pub fn reset() -> Result<AgentLimits, String> {
    save(&AgentLimits::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_positive() {
        let d = AgentLimits::default();
        assert_eq!(d.tool_calls(), Some(DEFAULT_TOOL_CALL_LIMIT as usize));
        assert_eq!(d.heavy_tool_calls(), Some(DEFAULT_HEAVY_TOOL_CALL_LIMIT as usize));
        assert_eq!(d.repeat_calls(), Some(DEFAULT_REPEAT_CALL_LIMIT as usize));
        assert_eq!(d.tool_rounds(), Some(DEFAULT_TOOL_ROUNDS as usize));
        assert_eq!(d.sub_agent_rounds(), Some(DEFAULT_SUB_AGENT_ROUNDS as usize));
        assert_eq!(d.task_duration_secs(), Some(DEFAULT_TASK_DURATION_MINUTES as u64 * 60));
        assert_eq!(d.blacklist_fail_threshold(), Some(DEFAULT_BLACKLIST_FAIL_THRESHOLD as usize));
        assert_eq!(d.repeat_edit_threshold(), Some(DEFAULT_REPEAT_EDIT_THRESHOLD as usize));
        assert_eq!(d.stall_tool_threshold(), Some(DEFAULT_STALL_TOOL_THRESHOLD as usize));
        assert_eq!(d.build_fail_converge_threshold(), Some(DEFAULT_BUILD_FAIL_CONVERGE_THRESHOLD as usize));
        assert_eq!(d.goal_reinject_interval(), Some(DEFAULT_GOAL_REINJECT_INTERVAL as usize));
    }

    #[test]
    fn zero_and_neg_one_mean_unlimited() {
        let mut l = AgentLimits {
            tool_call_limit: 0,
            heavy_tool_call_limit: -1,
            tool_rounds: 0,
            ..AgentLimits::default()
        };
        l.normalize().unwrap();
        assert_eq!(l.tool_calls(), None);
        assert_eq!(l.heavy_tool_calls(), None);
        assert_eq!(l.tool_rounds(), None);
        // 其余字段保持默认生效
        assert_eq!(l.repeat_calls(), Some(DEFAULT_REPEAT_CALL_LIMIT as usize));
    }

    #[test]
    fn invalid_values_rejected() {
        let mut l = AgentLimits { tool_call_limit: -2, ..AgentLimits::default() };
        assert!(l.normalize().is_err());
        l.tool_call_limit = MAX_LIMIT_VALUE + 1;
        assert!(l.normalize().is_err());
        l.tool_call_limit = MAX_LIMIT_VALUE;
        assert!(l.normalize().is_ok());
    }

    #[test]
    fn serde_roundtrip() {
        let mut l = AgentLimits { tool_rounds: -1, ..AgentLimits::default() };
        l.normalize().unwrap();
        let raw = serde_json::to_string(&l).unwrap();
        let back: AgentLimits = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, l);
    }
}
