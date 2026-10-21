import { describe, it, expect, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { BatchSendDialog } from './HomeDialogs'

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (k: string) => k }) }))

describe('BatchSendDialog', () => {
  it('按行解析后提交，返回全部成功时保持原样（由外层关闭）', async () => {
    const onSubmit = vi.fn().mockResolvedValue({ ok: 2, failed: [] })
    render(<BatchSendDialog initial={'第一行\n\n第二行'} onClose={() => {}} onSubmit={onSubmit} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.batchSend' }))
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(['第一行', '第二行']))
    expect(screen.getByRole('textbox')).toHaveValue('第一行\n\n第二行')
  })

  it('有失败行时只保留失败行，用户可修改后重试', async () => {
    const onSubmit = vi.fn().mockResolvedValue({ ok: 1, failed: ['第二行'] })
    render(<BatchSendDialog initial={'第一行\n第二行'} onClose={() => {}} onSubmit={onSubmit} />)
    fireEvent.click(screen.getByRole('button', { name: 'home.batchSend' }))
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveValue('第二行'))
  })

  it('Ctrl+Enter 提交编辑后的最新文本（行数不变也不能是旧闭包）', async () => {
    const onSubmit = vi.fn().mockResolvedValue({ ok: 1, failed: [] })
    render(<BatchSendDialog initial="旧内容" onClose={() => {}} onSubmit={onSubmit} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '新内容' } })
    fireEvent.keyDown(window, { key: 'Enter', ctrlKey: true })
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(['新内容']))
  })
})
