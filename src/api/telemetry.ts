import { invokeWithError } from './invoke'

/**
 * 本地使用统计的 IPC 封装
 *
 * 与 `utils/telemetry.ts` 的区别：**这里不做任何开关判断**——
 * 本文件是「和后端说话」的薄层，判断在埋点那一侧（见 telemetry.ts 的说明）。
 * 这样切开关后想验证「后端真的关着」，可以直接用 `recordUsageEventStrict` 打到后端。
 */

export interface UsageEventSummary {
  event_name: string
  count: number
  last_seen_at: number
}

/** 统计开关是否开启（settings 表的 telemetry_enabled，键不存在 = 关） */
export const usageStatsEnabled = () => invokeWithError<boolean>('usage_stats_enabled')

/** 开关设置。**关闭时后端会连带清空已记录数据** */
export const setUsageStatsEnabled = (enabled: boolean) =>
  invokeWithError<void>('set_usage_stats_enabled', { enabled })

/** 记录一条事件（正常路径，后端对失败静默） */
export const recordUsageEvent = (eventName: string, props?: Record<string, unknown>) =>
  invokeWithError<void>('record_usage_event', {
    eventName,
    props: props ? JSON.stringify(props) : undefined,
  })

/** 记录一条事件并把后端错误抛回（仅供设置页自检 / 契约测试） */
export const recordUsageEventStrict = (eventName: string, props?: Record<string, unknown>) =>
  invokeWithError<void>('record_usage_event_strict', {
    eventName,
    props: props ? JSON.stringify(props) : undefined,
  })

/** 汇总：按事件名聚合计数；sinceTs 传 0 表示不限 */
export const usageStatsSummary = (sinceTs = 0) =>
  invokeWithError<UsageEventSummary[]>('usage_stats_summary', { sinceTs })

/** 总条数 */
export const usageStatsTotal = () => invokeWithError<number>('usage_stats_total')

/** 清空全部记录（不改变开关状态） */
export const clearUsageStats = () => invokeWithError<void>('clear_usage_stats')

/** 后端白名单（设置页自检用它验证「前后端事件名没漂移」） */
export const usageEventNames = () => invokeWithError<string[]>('usage_event_names')
