import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { ToolRunGroup, ToolRunRow } from './toolRuns'
import type { ToolRun } from '../../stores/projectStore'

// 组件内 i18n 文案只用于展示；测试直接断言 key（t 原样返回），避免依赖完整 i18n 初始化
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}))

const run = (over: Partial<ToolRun> = {}): ToolRun => ({
  id: 'r1',
  tool: 'read_file',
  args: '{"path":"a.txt"}',
  status: 'done',
  output: '文件内容',
  ...over,
})

describe('ToolRunGroup', () => {
  it('折叠态展示工具名与完成计数', () => {
    render(<ToolRunGroup runs={[run(), run({ id: 'r2', tool: 'write_file' })]} />)
    expect(screen.getByText(/write_file/)).toBeInTheDocument() // 折叠态仅显示最后一次调用
    expect(screen.getByText(/home\.toolCalls/)).toBeInTheDocument()
    expect(screen.getByText(/home\.toolDone/)).toBeInTheDocument()
    expect(screen.getByText(/\u00d72/)).toBeInTheDocument() // ×2 完成计数
  })

  it('运行中状态显示运行中文案', () => {
    render(<ToolRunGroup runs={[run({ status: 'running', startedAt: Date.now() })]} />)
    expect(screen.getByText(/home\.toolRunning/)).toBeInTheDocument()
  })

  it('点击标题展开工具明细行', () => {
    const { container } = render(<ToolRunGroup runs={[run()]} />)
    expect(container.querySelectorAll('button')).toHaveLength(1) // 折叠态仅组标题按钮
    fireEvent.click(screen.getByRole('button'))
    // 展开后多出明细行（工具名出现两次：组标题 + 明细行）
    expect(screen.getAllByText(/read_file/).length).toBeGreaterThan(1)
  })
})

describe('ToolRunRow', () => {
  it('命令工具默认展开输出', () => {
    render(<ToolRunRow run={run({ tool: 'run_command' })} />)
    expect(screen.getByText('文件内容')).toBeInTheDocument()
  })

  it('非命令工具点击后展开输出', () => {
    const { container } = render(<ToolRunRow run={run()} />)
    expect(container.textContent).not.toContain('文件内容')
    fireEvent.click(screen.getAllByRole('button')[0])
    expect(screen.getByText('文件内容')).toBeInTheDocument()
  })
})
