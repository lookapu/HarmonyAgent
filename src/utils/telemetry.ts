import { recordUsageEvent, setUsageStatsEnabled } from '../api/telemetry'
import { STORAGE_KEYS } from '../constants'
import { getItem, setItem } from './storage'

/**
 * 功能使用埋点
 *
 * ## 承诺
 *
 * **默认关闭。** 开关是 localStorage 的 `telemetry:enabled`，
 * 未设置 = 关闭，此时 `track()` 连 invoke 都不发——不是「发了但后端丢弃」。
 * 两层都拒绝是刻意的：前端层省掉 IPC 开销，后端层保证**即使有人绕过前端
 * 直接调 command 也写不进数据**（见 commands/telemetry.rs）。
 *
 * **只落本地。** 本文件没有任何网络调用；整个项目也没有「上报」command。
 * 想外发数据必须新写一条 command 并改文档——那是一次显式的决定，不是默认行为。
 *
 * ## 为什么没有节流
 *
 * 考虑过同事件 N 毫秒内只记一次（节流），但那会让「次数」这个指标失真：
 * 面板上显示 3 次而实际触发 300 次，比不显示更糟。
 * 靠后端的行数上限（保留最近 20000 条）控制体量，而不是靠采样。
 *
 * ## 事件名必须与后端白名单一致
 *
 * `queries::EVENT_NAMES`。不一致时后端会**静默丢弃**（正常路径不抛错），
 * 于是埋点看起来接好了却什么都不记。故本文件的 `EventName` 类型
 * 直接从后端白名单派生（见下方 `EVENT_NAMES`），新增事件时两边一起改。
 */

/** 与后端 `queries::EVENT_NAMES` 一一对应。**改动必须同步** */
export const EVENT_NAMES = [
  'conversation_created',
  'agent_run_finished',
  'agent_run_failed',
  'message_regenerated',
  'message_edited',
  'message_feedback',
  'context_compacted',
  'page_view',
  'command_palette_used',
  'onboarding_step_viewed',
  'onboarding_finished',
  'onboarding_skipped',
  'settings_changed',
  'tool_used',
  'tool_denied',
  'project_added',
  'build_finished',
  'build_failed',
  'provider_configured',
  'provider_tested',
  'model_switched',
  'export_run',
  'import_run',
] as const

export type EventName = (typeof EVENT_NAMES)[number]

/** 统计是否已开启（前端侧视角） */
export function isTrackingEnabled(): boolean {
  return getItem(STORAGE_KEYS.TELEMETRY_ENABLED) === '1'
}

/**
 * 路由路径 → 低基数页面名
 *
 * **为什么要归一化**：直接记 `location.pathname` 会让 404 页面变成**无界**的
 * props 值——用户（或自动化测试）随便敲一个地址就是一条新「页面」。
 * 而 props 的用途是聚合，高基数等于把统计表变成日志表。
 *
 * 规则：只取第一段，且必须是小写字母/连字符组成的短词，否则记 `other`。
 * 取第一段是因为本项目的页面都是单段路由（`/providers`），将来出现
 * `/providers/xxx` 详情页时会归到同一类——这是**想要的**：
 * 「 providers 页被进了多少次」比「providers/1 和 providers/2 各多少次」有用。
 *
 * 刻意**不**维护一份「已知路由表」来白名单化：新增页面时忘记同步就会静默
 * 记成 `other`，而 `other` 是**看不出是漏同步还是真有异常**的值。
 * 形状约束（单个短词）本身就已经把基数限制住了。
 */
export function normalizePageName(pathname: string): string {
  const first = pathname.split('/').filter(Boolean)[0] ?? ''
  if (first === '') return 'home'
  return /^[a-z][a-z-]{0,23}$/.test(first) ? first : 'other'
}

/**
 * 记录一条使用事件
 *
 * 永不抛错、永不 await：埋点在正常路径上，失败不影响用户操作。
 * 开关未开时直接 return，连 IPC 都不发。
 */
export function track(eventName: EventName, props?: Record<string, unknown>): void {
  if (!isTrackingEnabled()) return
  // props 只放**低基数、无隐私**的值：页面名、工具名、状态码。
  // 不要放路径、文件名、用户名、prompt 片段——它们既无统计价值又是隐私负担。
  void recordUsageEvent(eventName, props).catch(() => {})
}

/** 路由切换时记录 `page_view`，props 为归一化后的页面名 */
export function trackPageView(pathname: string): void {
  track('page_view', { page: normalizePageName(pathname) })
}

/**
 * 开关设置
 *
 * 前后端各存一份：前端存 localStorage 供同步判断（省 IPC），
 * 后端存 settings 表作为**权威开关**。两者都要写——
 * 只写前端的话，绕过 UI 直接调后端仍能落库，承诺就不成立了。
 *
 * **顺序是「后端先写、成功才写 localStorage」**，不能反过来。
 * 反过来的话，后端写失败时 localStorage 已经变成新值：
 * 前端认为开着、实际一条也记不进（后端在关闭时静默丢弃），
 * 而下次进设置页 `isTrackingEnabled()` 读到的又是「新值」——
 * 于是界面显示「已开启」而后端是关的，**两边永久性地对不上**。
 * 调用方拿到异常后只需回滚 UI 状态，不用再管 localStorage。
 */
export async function setTrackingEnabled(enabled: boolean): Promise<void> {
  await setUsageStatsEnabled(enabled)
  setItem(STORAGE_KEYS.TELEMETRY_ENABLED, enabled ? '1' : '0')
}
