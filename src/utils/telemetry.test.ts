import { describe, it, expect, vi, beforeEach } from 'vitest'
import { STORAGE_KEYS } from '../constants'

/**
 * 埋点开关的三条不变量：
 * 1. **默认关闭时连 IPC 都不发**（不是「发了后端丢」）——省开销，也少一层可能被质疑的落库。
 * 2. **埋点失败不影响用户操作**：`track()` 永不抛错、永不 await。
 * 3. **props 低基数**：路由名归一化，避免 404 把统计表变成日志表。
 *
 * 第 3 条最容易在半年后以「面板上 4000 个页面」的形式爆出来，
 * 而爆出来时已经无从判断哪些是真实访问、哪些是乱敲的地址。
 */

const recordUsageEvent = vi.fn(async () => {})
const setUsageStatsEnabled = vi.fn(async () => {})
vi.mock('../api/telemetry', () => ({
  recordUsageEvent: (...a: unknown[]) => recordUsageEvent(...(a as [])),
  setUsageStatsEnabled: (...a: unknown[]) => setUsageStatsEnabled(...(a as [])),
}))

beforeEach(() => {
  localStorage.clear()
  recordUsageEvent.mockClear()
  setUsageStatsEnabled.mockClear()
  setUsageStatsEnabled.mockResolvedValue(undefined)
})

const setEnabled = (v: boolean) =>
  localStorage.setItem(STORAGE_KEYS.TELEMETRY_ENABLED, v ? '1' : '0')

describe('normalizePageName', () => {
  it('根路径记为 home 而不是空串——空串在面板上无法与「未记录」区分', async () => {
    const { normalizePageName } = await import('./telemetry')
    expect(normalizePageName('/')).toBe('home')
    expect(normalizePageName('')).toBe('home')
  })

  it('只取第一段，详情页归到所属功能', async () => {
    const { normalizePageName } = await import('./telemetry')
    expect(normalizePageName('/providers')).toBe('providers')
    expect(normalizePageName('/providers/abc-123')).toBe('providers')
    expect(normalizePageName('/team-sharing/')).toBe('team-sharing')
  })

  it('形状不合法的一律归 other，防止高基数 props', async () => {
    const { normalizePageName } = await import('./telemetry')
    expect(normalizePageName('/Providers')).toBe('other')
    expect(normalizePageName('/中文路径')).toBe('other')
    expect(normalizePageName('/' + 'a'.repeat(40))).toBe('other')
    expect(normalizePageName('/9lives')).toBe('other')
  })
})

describe('track 开关', () => {
  it('未开启时不发 invoke（默认关闭必须是真的不发）', async () => {
    const { track, isTrackingEnabled } = await import('./telemetry')
    expect(isTrackingEnabled()).toBe(false)
    track('page_view', { page: 'home' })
    expect(recordUsageEvent).not.toHaveBeenCalled()
  })

  it('显式关闭同样不发', async () => {
    setEnabled(false)
    const { track, isTrackingEnabled } = await import('./telemetry')
    expect(isTrackingEnabled()).toBe(false)
    track('page_view', { page: 'home' })
    expect(recordUsageEvent).not.toHaveBeenCalled()
  })

  it('开启后正常记录，且页面名已归一化', async () => {
    setEnabled(true)
    const { trackPageView } = await import('./telemetry')
    expect(trackPageView('/成本')).toBeUndefined()
    expect(recordUsageEvent).toHaveBeenCalledWith('page_view', { page: 'other' })

    recordUsageEvent.mockClear()
    trackPageView('/cost')
    expect(recordUsageEvent).toHaveBeenCalledWith('page_view', { page: 'cost' })
  })
})

describe('track 容错', () => {
  it('后端报错时静默吞掉，不冒泡到调用方', async () => {
    setEnabled(true)
    recordUsageEvent.mockRejectedValueOnce(new Error('后端炸了'))
    const { track } = await import('./telemetry')
    expect(() => track('tool_used', { tool: 'read' })).not.toThrow()
  })
})

describe('setTrackingEnabled 的写入顺序', () => {
  it('成功时后端与 localStorage 一起变', async () => {
    const { setTrackingEnabled, isTrackingEnabled } = await import('./telemetry')
    await setTrackingEnabled(true)
    expect(setUsageStatsEnabled).toHaveBeenCalledWith(true)
    expect(isTrackingEnabled()).toBe(true)

    await setTrackingEnabled(false)
    expect(setUsageStatsEnabled).toHaveBeenLastCalledWith(false)
    expect(isTrackingEnabled()).toBe(false)
  })

  // 顺序写成「先写 localStorage 再调后端」时，后端失败会留下一个
  // 「前端说开着、后端说关着」的永久不一致：界面上显示已开启，
  // 实际一条也记不进（后端静默丢弃），且下次进设置页读到的还是那个新值。
  it('后端写失败时抛错，且 localStorage 保持原值', async () => {
    setEnabled(false)
    setUsageStatsEnabled.mockRejectedValueOnce(new Error('db locked'))
    const { setTrackingEnabled, isTrackingEnabled } = await import('./telemetry')
    await expect(setTrackingEnabled(true)).rejects.toThrow('db locked')
    expect(isTrackingEnabled()).toBe(false)
  })

  it('后端写失败时也不能抛出去给 UI 之外的地方——由调用方决定怎么提示', async () => {
    setEnabled(true)
    setUsageStatsEnabled.mockRejectedValueOnce(new Error('db locked'))
    const { setTrackingEnabled, isTrackingEnabled } = await import('./telemetry')
    await expect(setTrackingEnabled(false)).rejects.toThrow()
    expect(isTrackingEnabled()).toBe(true)
  })
})
