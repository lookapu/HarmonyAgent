import { describe, it, expect, vi, beforeEach } from 'vitest'
import type { Mock } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Modal } from './Modal'

// 组件内文案只用于展示；测试直接断言 key（t 原样返回），避免依赖完整 i18n 初始化
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (k: string) => k }),
}))

const backdrop = (): HTMLElement => screen.getByRole('dialog').parentElement as HTMLElement

describe('Modal', () => {
  // 必须写成 Mock<() => void>：裸 ReturnType<typeof vi.fn> 是
  // Mock<Procedure | Constructable>，赋不进 Modal 的 onClose?: () => void
  let onClose: Mock<() => void>

  beforeEach(() => {
    onClose = vi.fn<() => void>()
  })

  it('open=false 时不渲染任何内容', () => {
    render(
      <Modal open={false} onClose={onClose} title="t">
        body
      </Modal>,
    )
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('可关闭模态：role/aria 齐备，标题被 aria-labelledby 指向', () => {
    render(
      <Modal open onClose={onClose} title="删除项目">
        body
      </Modal>,
    )
    const dialog = screen.getByRole('dialog')
    expect(dialog).toHaveAttribute('aria-modal', 'true')
    expect(dialog).toHaveAttribute('aria-labelledby', screen.getByText('删除项目').id)
  })

  it('可关闭模态：Esc / 遮罩点击 / 关闭按钮 三条路径都能关', () => {
    render(
      <Modal open onClose={onClose} title="t">
        body
      </Modal>,
    )
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledTimes(1)

    fireEvent.mouseDown(backdrop())
    expect(onClose).toHaveBeenCalledTimes(2)

    fireEvent.click(screen.getByLabelText('common.close'))
    expect(onClose).toHaveBeenCalledTimes(3)
  })

  it('点击面板内部不关闭', () => {
    render(
      <Modal open onClose={onClose} title="t">
        <p>panel content</p>
      </Modal>,
    )
    fireEvent.mouseDown(screen.getByText('panel content'))
    fireEvent.mouseDown(screen.getByRole('dialog'))
    expect(onClose).not.toHaveBeenCalled()
  })

  it('阻塞式模态（不传 onClose）：无 Esc、遮罩不可点、不渲染关闭按钮', () => {
    render(
      <Modal open title="工具权限审核">
        <p>must decide</p>
      </Modal>,
    )
    fireEvent.keyDown(window, { key: 'Escape' })
    fireEvent.mouseDown(backdrop())
    expect(screen.queryByLabelText('common.close')).toBeNull()
    // 仍应正常渲染内容——阻塞的是关闭路径，不是内容
    expect(screen.getByText('must decide')).toBeInTheDocument()
  })

  it('嵌套时 Esc 只关最上层', () => {
    const onOuter = vi.fn()
    const onInner = vi.fn()
    render(
      <Modal open onClose={onOuter} title="outer">
        outer body
      </Modal>,
    )
    render(
      <Modal open onClose={onInner} title="inner">
        inner body
      </Modal>,
    )
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(onInner).toHaveBeenCalledTimes(1)
    expect(onOuter).not.toHaveBeenCalled()
  })
})
