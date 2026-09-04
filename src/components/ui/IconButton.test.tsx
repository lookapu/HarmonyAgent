import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { IconButton } from './IconButton'

describe('IconButton', () => {
  it('label 必填，同时落到 aria-label 与 title', () => {
    render(<IconButton icon="close" label="关闭" />)
    const btn = screen.getByRole('button')
    expect(btn).toHaveAttribute('aria-label', '关闭')
    expect(btn).toHaveAttribute('title', '关闭')
  })

  it('图标对读屏隐藏，避免 <img alt> 与 aria-label 重复播报', () => {
    render(<IconButton icon="close" label="关闭" />)
    expect(screen.getByRole('button').firstElementChild).toHaveAttribute('aria-hidden', 'true')
  })

  // Tailwind preflight 把元素重置成 border: 0 solid，被替换的手写按钮本就无边框。
  // 裸按钮一旦带上 border（哪怕 transparent），每颗都会悄悄长大 2px。
  it('裸按钮不带 border，尺寸只由 pad + 图标决定', () => {
    render(<IconButton icon="close" label="关闭" />)
    const btn = screen.getByRole('button')
    expect(btn.className).not.toMatch(/\bborder\b/)
    expect(btn).toHaveClass('p-1')
  })

  it('box 才给边框与卡片底（模态头部、成组工具栏用）', () => {
    render(<IconButton icon="close" label="关闭" box />)
    expect(screen.getByRole('button')).toHaveClass('border', 'border-[var(--border)]')
  })

  it('hoverTone 只换底色，不换文字色——Icon 是 <img>+filter，文字色是死类', () => {
    render(<IconButton icon="delete" label="删除" hoverTone="danger" />)
    const btn = screen.getByRole('button')
    expect(btn).toHaveClass('hover:bg-[var(--danger-50)]')
    expect(btn.className).not.toMatch(/hover:text-/)
  })

  it('默认 type=button，不会在将来的表单里意外提交', () => {
    render(<IconButton icon="close" label="关闭" />)
    expect(screen.getByRole('button')).toHaveAttribute('type', 'button')
  })
})
