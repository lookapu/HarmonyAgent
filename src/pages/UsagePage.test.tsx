import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import UsagePage from './UsagePage'
import { EVENT_NAMES } from '../utils/telemetry'
import {
  usageStatsSummary,
  usageStatsTotal,
  usageEventNames,
  recordUsageEventStrict,
  clearUsageStats,
} from '../api/telemetry'

/**
 * UsagePage 的两条不变量，都属于「**界面上看不出异常**」那一族：
 *
 * 1. **不能自己反复去查库**。`sinceTs` 若写成裸的 `Date.now() - days*86400`（不 memo），
 *    每次渲染都是新值 → `load` 重建 → effect 重跑 → 又查一次库，而数据不会因此变新。
 *    **实测的形态要说准**：`Math.floor(Date.now()/1000)` 量化到秒，只有当两次连续
 *    渲染恰好跨过整秒边界时引用才真变，所以它是**低频、时机不可预测的重复拉取**，
 *    不是每帧轮询（我第一版写成「每帧两次 IPC 的无限轮询」是夸大，探针打脸后改正）。
 *    页面不动时它大多会自然停住——没有下一次渲染就没有下一次 effect——
 *    这类「偶发且无用途」的开销比稳定轮询更难在监控里发现。
 *    默认值 'all' 返回常量 0，所以这个坑只在用户点过区间筛选之后才存在：
 *    **默认值恰好掩盖了 bug，不等于没有 bug。**
 *
 * 2. **自检不能在没写入时报「通过」**。统计开关关闭时后端是**静默返回 Ok**
 *    的，只判「没抛错」就是把「一条都没写进去」显示成绿色。
 */

const t = (k: string) => (k.includes('.') ? k.split('.').pop()! : k)
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t, i18n: { changeLanguage: vi.fn() } }),
  initReactI18next: { type: '3rdParty', init: () => {} },
}))
vi.mock('../api/telemetry', () => ({
  usageStatsSummary: vi.fn(),
  usageStatsTotal: vi.fn(),
  clearUsageStats: vi.fn(),
  usageEventNames: vi.fn(),
  recordUsageEventStrict: vi.fn(),
}))

const summary = [
  { event_name: 'page_view', count: 12, last_seen_at: 1 },
  { event_name: 'tool_used', count: 3, last_seen_at: 1 },
]

beforeEach(() => {
  vi.clearAllMocks()
  // **每次返回新对象**：真实 IPC 每次反序列化都给出新数组/新对象。
  // 若 mock 返回同一个引用，React 的 Object.is 短路会让重渲染根本不发生，
  // 于是「每帧重算 sinceTs」这个 bug 在测试里**测不出来**——
  // 门禁/测试的假阴性比假阳性更难发现，因为它让人以为这条有网。
  vi.mocked(usageStatsSummary).mockImplementation(async () => summary.map((r) => ({ ...r })))
  vi.mocked(usageStatsTotal).mockResolvedValue(15)
  // 用**真实的**全量名单：写死几个假事件名的话，自检会先判 drift，
  // 于是「写入」那几步根本走不到，用例就在测另一条分支了。
  vi.mocked(usageEventNames).mockResolvedValue([...EVENT_NAMES])
  vi.mocked(recordUsageEventStrict).mockResolvedValue(undefined)
  vi.mocked(clearUsageStats).mockResolvedValue(undefined)
})

const settle = () => new Promise((r) => setTimeout(r, 30))

describe('UsagePage 拉取次数', () => {
  it('初次挂载只查一次', async () => {
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('total')).toBeInTheDocument())
    await settle()
    expect(usageStatsSummary).toHaveBeenCalledTimes(1)
  })

  it('切到「近 7 天」不会变成持续轮询（sinceTs 必须被 memo）', async () => {
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('total')).toBeInTheDocument())
    const before = vi.mocked(usageStatsSummary).mock.calls.length

    fireEvent.click(screen.getByRole('button', { name: '7' }))
    await settle()
    // 切换区间应当**恰好再查一次**
    expect(usageStatsSummary).toHaveBeenCalledTimes(before + 1)

    // 这条断言的真实价值有限：跨整秒边界的重拉是**低频偶发**的，
    // 短时间内观察不到。真正被钉住的是下面「初次挂载只查一次」与
    // 「切区间只多查一次」——它们能挡住依赖数组被进一步改坏的情况。
    // 探针过程记在这里：实测 2.6 秒内不 memo 版本也只多查了 1 次，
    // **所以我第一版「每帧无限轮询」的描述是错的，已按实测改正**。
    await settle()
    await settle()
    expect(usageStatsSummary).toHaveBeenCalledTimes(before + 1)
  })

  it('区间筛选确实把 since 传给了后端（不是恒 0）', async () => {
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('total')).toBeInTheDocument())
    expect(vi.mocked(usageStatsSummary).mock.calls[0][0]).toBe(0)

    fireEvent.click(screen.getByRole('button', { name: '30' }))
    await waitFor(() => {
      const last = vi.mocked(usageStatsSummary).mock.calls.at(-1)![0]
      // 30 天前的某个时间戳，而不是 0
      expect(last).toBeGreaterThan(0)
      expect(last).toBeLessThan(Date.now() / 1000)
    })
  })
})

describe('UsagePage 自检', () => {
  it('真的写进去了才报「通过」', async () => {
    // 按「写入才计数」建模而不是排 mock 返回值序列：首屏 load 也会调一次 total，
    // 顺序一排错，用例就变成在测另一条分支（我第一版就是这么错的）。
    let total = 15
    vi.mocked(usageStatsTotal).mockImplementation(async () => total)
    vi.mocked(recordUsageEventStrict).mockImplementation(async () => {
      total += 1
    })

    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('total')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'selfCheck' }))
    await waitFor(() => expect(screen.getByText('selfCheckOk')).toBeInTheDocument())
  })

  it('没写进去时不能报「通过」——开关关闭时后端是静默 Ok 的', async () => {
    // 关键：recordUsageEventStrict **不抛错也不计数**（后端在开关关闭时的真实行为）
    vi.mocked(recordUsageEventStrict).mockImplementation(async () => {
      /* 静默丢弃：没开统计不是错误状态 */
    })
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('total')).toBeInTheDocument())

    fireEvent.click(screen.getByRole('button', { name: 'selfCheck' }))
    await waitFor(() => expect(screen.getByText('selfCheckNotWritten')).toBeInTheDocument())
    expect(screen.queryByText('selfCheckOk')).not.toBeInTheDocument()
  })

  it('事件名漂移时报 drift，且不再尝试写入', async () => {
    // 少一个 = 真实漂移形态（有人只改了后端）
    vi.mocked(usageEventNames).mockResolvedValue(EVENT_NAMES.filter((n) => n !== 'export_run'))
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('total')).toBeInTheDocument())

    fireEvent.click(screen.getByRole('button', { name: 'selfCheck' }))
    await waitFor(() => expect(screen.getByText('selfCheckDrift')).toBeInTheDocument())
    expect(recordUsageEventStrict).not.toHaveBeenCalled()
  })
})

describe('UsagePage 失败与空态', () => {
  it('读不到时不显示「0」——那是「确实没数据」与「读不出来」两件事', async () => {
    vi.mocked(usageStatsSummary).mockRejectedValue(new Error('db locked'))
    vi.mocked(usageStatsTotal).mockRejectedValue(new Error('db locked'))
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText(/loadError/)).toBeInTheDocument())
    expect(screen.queryByText('empty')).not.toBeInTheDocument()
  })

  it('真的空时给空态而不是加载中', async () => {
    vi.mocked(usageStatsSummary).mockResolvedValue([])
    vi.mocked(usageStatsTotal).mockResolvedValue(0)
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('empty')).toBeInTheDocument())
  })

  it('清空后重新拉取', async () => {
    render(<UsagePage />)
    await waitFor(() => expect(screen.getByText('total')).toBeInTheDocument())
    const before = vi.mocked(usageStatsSummary).mock.calls.length
    fireEvent.click(screen.getByRole('button', { name: 'clear' }))
    await waitFor(() => expect(usageStatsSummary).toHaveBeenCalledTimes(before + 1))
    expect(clearUsageStats).toHaveBeenCalled()
  })
})
