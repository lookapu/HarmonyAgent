import { invoke } from '@tauri-apps/api/core'

/** Agent 护栏参数。字段为 0 或 -1 表示不限制（关闭该项护栏） */
export interface AgentLimits {
  /** 单任务工具调用总预算 */
  tool_call_limit: number
  /** 重操作（build/deploy/run_command）单任务最多调用次数 */
  heavy_tool_call_limit: number
  /** 打转检测阈值：同一 (工具, 参数) 连续重复 N 次判定打转 */
  repeat_call_limit: number
  /** 主 Agent 工具轮次上限 */
  tool_rounds: number
  /** 单任务最长执行时长（分钟） */
  task_duration_minutes: number
  /** 子 Agent 内部循环轮次上限 */
  sub_agent_rounds: number
  /** 失败动作黑名单阈值 */
  blacklist_fail_threshold: number
  /** 同文件连续编辑后强制构建验证的阈值 */
  repeat_edit_threshold: number
  /** 失速检测阈值（无实质进展的工具调用次数） */
  stall_tool_threshold: number
  /** 构建连续失败收敛阈值 */
  build_fail_converge_threshold: number
  /** 目标锚定重注入间隔（每 N 轮工具调用） */
  goal_reinject_interval: number
}

/** 读取当前生效的护栏配置 */
export const getAgentLimits = () => invoke<AgentLimits>('get_agent_limits')
/** 保存护栏配置（0/-1 表示不限制）；返回归一化后的配置 */
export const setAgentLimits = (limits: AgentLimits) => invoke<AgentLimits>('set_agent_limits', { limits })
/** 恢复全部护栏参数为默认值 */
export const resetAgentLimits = () => invoke<AgentLimits>('reset_agent_limits')
