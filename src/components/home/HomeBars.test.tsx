import { describe, it, expect, vi, beforeEach } from 'vitest'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { TFunction } from 'i18next'
import { buildGuardNudgeText, GuardBar } from './HomeBars'
import { useGuardStore, type GuardState } from '../../stores/guardStore'

// 文案只用于展示：t 原样返回 key，断言不依赖完整 i18n 初始化
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (k: string) => k }) }))

const conv = 'conv-1'

function setGuard(partial: Partial<GuardState>) {
  useGuardStore.setState({
    guards: {
      [conv]: {
        stage: 2,
        sinceProgress: 24,
        stalledBytes: 160 * 1024,
        threshold: 10,
        profile: { repeats: [], topTool: null },
        at: 0,
        ...partial,
      },
    },
  })
}

beforeEach(() => {
  useGuardStore.setState({ guards: {} })
})

describe('GuardBar 失速提示条', () => {
  it('没有失速状态时不渲染（不留空占位）', () => {
    const { container } = render(<GuardBar convId={conv} />)
    expect(container.firstChild).toBeNull()
  })

  it('stage=0（已恢复）时不渲染', () => {
    setGuard({ stage: 0 })
    const { container } = render(<GuardBar convId={conv} />)
    expect(container.firstChild).toBeNull()
  })

  it('有重复动作时点名工具、目标与次数', () => {
    setGuard({
      profile: {
        repeats: [{ tool: 'read_file', target: 'src/main.ets', count: 9 }],
        topTool: { tool: 'read_file', count: 9 },
      },
    })
    render(<GuardBar convId={conv} />)
    expect(screen.getByText('home.guardRepeats')).toBeInTheDocument()
    expect(screen.getByText('read_file')).toBeInTheDocument()
    expect(screen.getByText(/src\/main\.ets/)).toBeInTheDocument()
    expect(screen.getByText(/×9/)).toBeInTheDocument()
  })

  it('没有重复动作时退到主导工具（跨目标聚合）', () => {
    setGuard({
      profile: { repeats: [], topTool: { tool: 'grep_files', count: 24 } },
    })
    render(<GuardBar convId={conv} />)
    expect(screen.getByText('home.guardTopTool')).toBeInTheDocument()
    expect(screen.getByText('grep_files')).toBeInTheDocument()
    expect(screen.getByText(/×24/)).toBeInTheDocument()
    // 没有重复动作时不该出现「重复最多」这一栏
    expect(screen.queryByText('home.guardRepeats')).toBeNull()
  })

  it('画像整个缺失（旧后端 / 字段对不上）时不崩，只显示级别与字节', () => {
    setGuard({ profile: undefined as unknown as GuardState['profile'] })
    render(<GuardBar convId={conv} />)
    expect(screen.getByText('home.guardTitle')).toBeInTheDocument()
    expect(screen.queryByText('home.guardRepeats')).toBeNull()
    expect(screen.queryByText('home.guardTopTool')).toBeNull()
  })

  it('过长的目标被截断（不撑破提示条）', () => {
    const long = 'a'.repeat(80)
    setGuard({
      profile: {
        repeats: [{ tool: 'read_file', target: long, count: 3 }],
        topTool: null,
      },
    })
    render(<GuardBar convId={conv} />)
    expect(screen.queryByText(long)).toBeNull()
    expect(screen.getByText(/…/)).toBeInTheDocument()
  })
})

describe('GuardBar 换个思路', () => {
  /** 带插值的最小 t：把 key 与参数都拼进返回值，便于断言正文内容 */
  const tt = ((key: string, opts?: Record<string, unknown>) =>
    opts ? `${key}|${JSON.stringify(opts)}` : key) as unknown as TFunction

  it('正文点名重复动作（工具 + 目标 + 次数），并带上连续次数', () => {
    const text = buildGuardNudgeText(tt, 24, {
      repeats: [
        { tool: 'read_file', target: 'src/main.ets', count: 9 },
        { tool: 'list_dir', target: null, count: 6 },
      ],
      topTool: { tool: 'read_file', count: 15 },
    })
    expect(text).toContain('"count":24')
    expect(text).toContain('read_file(src/main.ets) × 9')
    expect(text).toContain('list_dir × 6')
  })

  it('没有重复动作时退到主导工具', () => {
    const text = buildGuardNudgeText(tt, 24, {
      repeats: [],
      topTool: { tool: 'grep_files', count: 24 },
    })
    expect(text).toContain('grep_files')
    expect(text).toContain('"count":24')
  })

  it('画像缺失时只保留连续次数，不留下空括号或 undefined', () => {
    const text = buildGuardNudgeText(tt, 12, undefined)
    expect(text).toContain('"count":12')
    expect(text).toContain('"detail":""')
    expect(text).not.toContain('undefined')
  })

  it('没有回调时不渲染按钮（纯展示用法不变）', () => {
    setGuard({})
    render(<GuardBar convId={conv} />)
    expect(screen.queryByRole('button', { name: 'home.guardNudge' })).toBeNull()
  })

  it('点击后把指令交给回调，成功后同一级别只提醒一次', async () => {
    setGuard({
      profile: {
        repeats: [{ tool: 'read_file', target: 'src/main.ets', count: 9 }],
        topTool: null,
      },
    })
    const onNudge = vi.fn().mockResolvedValue(undefined)
    render(<GuardBar convId={conv} onNudge={onNudge} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.guardNudge' }))
    await waitFor(() => expect(onNudge).toHaveBeenCalledTimes(1))
    expect(onNudge).toHaveBeenCalledWith('home.guardNudgeText')
    const done = await screen.findByRole('button', { name: 'home.guardNudged' })
    expect(done).toBeDisabled()
    fireEvent.click(done)
    expect(onNudge).toHaveBeenCalledTimes(1)
  })

  it('发送失败时按钮恢复可用，允许重试', async () => {
    setGuard({})
    const onNudge = vi.fn().mockRejectedValue(new Error('boom'))
    render(<GuardBar convId={conv} onNudge={onNudge} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.guardNudge' }))
    await waitFor(() => expect(onNudge).toHaveBeenCalledTimes(1))
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'home.guardNudge' })).toBeEnabled(),
    )
  })

  it('级别升级后重新可提醒（同一级别才去重）', async () => {
    setGuard({ stage: 2 })
    const onNudge = vi.fn().mockResolvedValue(undefined)
    render(<GuardBar convId={conv} onNudge={onNudge} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.guardNudge' }))
    await waitFor(() => expect(onNudge).toHaveBeenCalledTimes(1))
    act(() => setGuard({ stage: 3 }))
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'home.guardNudge' })).toBeEnabled(),
    )
  })
})
