import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { Segmented } from './Segmented'

const opts = [
  { value: 'dark' as const, label: '深色' },
  { value: 'light' as const, label: '浅色' },
  { value: 'auto' as const, label: '跟随系统' },
]

describe('Segmented', () => {
  it('当前值标记 aria-pressed，其余不带', () => {
    render(<Segmented options={opts} value="light" onChange={() => {}} label="主题" />)
    expect(screen.getByRole('button', { name: '浅色' })).toHaveAttribute(
      'aria-pressed',
      'true',
    )
    expect(screen.getByRole('button', { name: '深色' })).toHaveAttribute(
      'aria-pressed',
      'false',
    )
  })

  it('点击未选中项回调该值', () => {
    const onChange = vi.fn()
    render(<Segmented options={opts} value="dark" onChange={onChange} label="主题" />)
    fireEvent.click(screen.getByRole('button', { name: '跟随系统' }))
    expect(onChange).toHaveBeenCalledWith('auto')
  })

  it('点击已选中项也会回调（允许用户点当前项做「重置」语义）', () => {
    const onChange = vi.fn()
    render(<Segmented options={opts} value="auto" onChange={onChange} label="主题" />)
    fireEvent.click(screen.getByRole('button', { name: '跟随系统' }))
    expect(onChange).toHaveBeenCalledWith('auto')
  })

  // group + aria-label 是这个控件唯一的读屏入口，缺了就只剩一排无名的按钮
  it('容器带 role=group 与可访问名', () => {
    render(<Segmented options={opts} value="dark" onChange={() => {}} label="主题" />)
    expect(screen.getByRole('group', { name: '主题' })).toBeInTheDocument()
  })

  it('disabled 时全部不可点', () => {
    const onChange = vi.fn()
    render(
      <Segmented options={opts} value="dark" onChange={onChange} label="主题" disabled />,
    )
    const btn = screen.getByRole('button', { name: '浅色' })
    expect(btn).toBeDisabled()
    fireEvent.click(btn)
    expect(onChange).not.toHaveBeenCalled()
  })

  it('每个按钮都是 type=button，不会在将来的表单里意外提交', () => {
    render(<Segmented options={opts} value="dark" onChange={() => {}} label="主题" />)
    for (const b of screen.getAllByRole('button')) {
      expect(b).toHaveAttribute('type', 'button')
    }
  })
})
