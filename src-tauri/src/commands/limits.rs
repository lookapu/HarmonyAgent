//! Agent 护栏参数配置命令：设置页「工具限制」读写入口。
//!
//! 配置存于 settings 表（key = agent_limits），进程内全局缓存即时生效，
//! 无需重启。字段 0/-1 表示不限制（见 services::agent_limits）。

use crate::services::agent_limits::{self, AgentLimits};
use tauri::State;

use crate::db::DbState;

/// 读取当前生效的护栏配置（与设置页展示一致，0/-1 已归一化为 -1）
#[tauri::command]
pub fn get_agent_limits(_state: State<'_, DbState>) -> AgentLimits {
    agent_limits::current()
}

/// 保存护栏配置：校验（0/-1 归一化为不限制）→ 持久化 → 全局缓存即时生效。
/// 返回归一化后的配置，非法值返回错误信息。
#[tauri::command]
pub fn set_agent_limits(_state: State<'_, DbState>, limits: AgentLimits) -> Result<AgentLimits, String> {
    agent_limits::save(&limits)
}

/// 恢复全部护栏参数为默认值
#[tauri::command]
pub fn reset_agent_limits(_state: State<'_, DbState>) -> Result<AgentLimits, String> {
    agent_limits::reset()
}
