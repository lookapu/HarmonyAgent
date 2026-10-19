import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import { GuardBar } from './HomeBars'
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
