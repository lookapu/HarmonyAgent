import { describe, it, expect, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { EditMessageDialog } from './dialogs'
import type { ChatMessage } from '../../api/project'

// 组件内 i18n 文案只用于展示；断言 key（t 原样返回），避免依赖完整 i18n 初始化
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}))

const msg = (over: Partial<ChatMessage>): ChatMessage =>
  ({ id: 'm1', role: 'user', content: '内容', queued: 0, ...over }) as ChatMessage

describe('EditMessageDialog', () => {
  it('排队消息提示原地保存，不承诺重新执行任务', () => {
    render(<EditMessageDialog message={msg({ queued: 1 })} onCancel={() => {}} onSubmit={() => {}} />)
    expect(screen.getByText('home.editQueuedHint')).toBeInTheDocument()
    expect(screen.queryByText('home.editRerunHint')).not.toBeInTheDocument()
  })

  it('历史用户消息提示保存会重跑', () => {
    render(<EditMessageDialog message={msg({ queued: 0 })} onCancel={() => {}} onSubmit={() => {}} />)
    expect(screen.getByText('home.editRerunHint')).toBeInTheDocument()
    expect(screen.queryByText('home.editQueuedHint')).not.toBeInTheDocument()
  })

  it('助手消息不显示重跑/排队提示', () => {
    render(
      <EditMessageDialog
        message={msg({ role: 'assistant', content: '回复' })}
        onCancel={() => {}}
        onSubmit={() => {}}
      />,
    )
    expect(screen.queryByText('home.editRerunHint')).not.toBeInTheDocument()
    expect(screen.queryByText('home.editQueuedHint')).not.toBeInTheDocument()
  })
})
