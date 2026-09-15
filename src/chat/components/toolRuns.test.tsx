import { describe, it, expect, vi } from 'vitest'
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react'
import { getOtaApprovalRevoked, revokeOtaApproval } from '../../api/project'
import { ToolRunGroup, ToolRunRow } from './toolRuns'
import type { ToolRun } from '../../stores/projectStore'

// 组件内 i18n 文案只用于展示；测试直接断言 key（t 原样返回），避免依赖完整 i18n 初始化
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}))
vi.mock('../../api/project', () => ({ revokeOtaApproval: vi.fn(), getOtaApprovalRevoked: vi.fn().mockResolvedValue(false) }))

const run = (over: Partial<ToolRun> = {}): ToolRun => ({
  id: 'r1',
  tool: 'read_file',
  args: '{"path":"a.txt"}',
  status: 'done',
  output: '文件内容',
  ...over,
})

describe('ToolRunGroup', () => {
  it('旧调用的延迟撤销结果不会污染新调用卡片', async () => {
    let complete!: () => void
    vi.mocked(revokeOtaApproval).mockImplementationOnce(() => new Promise<void>((resolve) => { complete = resolve }))
    const { rerender } = render(<ToolRunRow run={run({ callId: 'old', tool: 'ota_pack', status: 'running' })} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.revokeOtaApproval' }))
    rerender(<ToolRunRow run={run({ id: 'new', callId: 'new', tool: 'ota_pack', status: 'running' })} />)
    await act(async () => { complete() })
    expect(screen.getByRole('button', { name: 'home.revokeOtaApproval' })).toBeEnabled()
    expect(screen.queryByRole('button', { name: 'home.otaApprovalRevoked' })).not.toBeInTheDocument()
  })
  it('重新挂载后从持久状态恢复已撤销提示', async () => {
    vi.mocked(getOtaApprovalRevoked).mockResolvedValueOnce(true)
    render(<ToolRunRow run={run({ callId: 'restored-call', tool: 'ota_pack', status: 'running' })} />)
    expect(await screen.findByRole('button', { name: 'home.otaApprovalRevoked' })).toBeDisabled()
    expect(getOtaApprovalRevoked).toHaveBeenCalledWith('restored-call')
  })
  it('缺少真实调用 ID 时不提供撤销按钮', () => {
    render(<ToolRunRow run={run({ tool: 'ota_pack', status: 'running' })} />)
    expect(screen.queryByRole('button', { name: 'home.revokeOtaApproval' })).not.toBeInTheDocument()
  })
  it('OTA 独立撤销发送精确调用 ID，成功后禁用按钮', async () => {
    vi.mocked(revokeOtaApproval).mockResolvedValueOnce(undefined)
    render(<ToolRunRow run={run({ id: 'tool-call-ota-call', callId: 'ota-call', tool: 'ota_pack', status: 'running' })} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.revokeOtaApproval' }))
    await waitFor(() => expect(revokeOtaApproval).toHaveBeenCalledWith('ota-call'))
    expect(await screen.findByRole('button', { name: 'home.otaApprovalRevoked' })).toBeDisabled()
    expect(screen.getByRole('status')).toHaveTextContent('home.otaRevokeNotice')
  })

  it('撤销失败可重试，不虚报成功', async () => {
    vi.mocked(revokeOtaApproval).mockRejectedValueOnce(new Error('database busy'))
    render(<ToolRunRow run={run({ callId: 'ota-call', tool: 'ota_pack', status: 'running' })} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.revokeOtaApproval' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('database busy')
    expect(screen.getByRole('button', { name: 'home.revokeOtaApproval' })).toBeEnabled()
  })
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
