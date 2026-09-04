import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Spinner, Skeleton } from './Spinner'

describe('Spinner', () => {
  it('默认 ring：2px 边、灰轨 + accent 顶边，直径走 inline style', () => {
    render(<Spinner size={32} />)
    const el = screen.getByRole('status')
    expect(el).toHaveClass('border-2', 'border-[var(--border-strong)]', 'border-t-[var(--accent)]', 'animate-spin')
    expect(el.style.width).toBe('32px')
    expect(el.style.height).toBe('32px')
  })

  // inline 是全库 12+ 处的既有写法：accent 轨 + 透明顶。若被 ring 的灰轨顶掉，
  // 主色就从 90% 的环缩成 10% 的环——「收敛」会变成「改色」，这条测试锁住它
  it('inline 是 accent 轨 + 透明顶边，绝不带 ring 的灰轨', () => {
    render(<Spinner variant="inline" size={10} />)
    const el = screen.getByRole('status')
    expect(el).toHaveClass('border', 'border-[var(--accent)]', 'border-t-transparent')
    expect(el).not.toHaveClass('border-2', 'border-[var(--border-strong)]')
    expect(el.style.width).toBe('10px')
  })

  // primary 按钮是 .btn-primary（accent-600 实心底），accent 顶边落在同色底上等于隐形，
  // 忙时只剩一个看着不动的灰圈。invert 必须对两个 variant 都换成白色系
  it('invert 在实心底色上换成白色系，两个 variant 都覆盖', () => {
    const { rerender } = render(<Spinner invert />)
    expect(screen.getByRole('status')).toHaveClass('border-white/40', 'border-t-white')
    expect(screen.getByRole('status')).not.toHaveClass('border-t-[var(--accent)]')
    rerender(<Spinner variant="inline" invert />)
    expect(screen.getByRole('status')).toHaveClass('border-white/70', 'border-t-transparent')
    expect(screen.getByRole('status')).not.toHaveClass('border-[var(--accent)]')
  })

  it('不传 label 就不写 aria-label，由调用方的可见文案说明上下文', () => {
    const { rerender } = render(<Spinner />)
    expect(screen.getByRole('status')).not.toHaveAttribute('aria-label')
    rerender(<Spinner label="正在加载会话" />)
    expect(screen.getByRole('status')).toHaveAttribute('aria-label', '正在加载会话')
  })

  it('className 只附加，不覆盖 variant 与尺寸类', () => {
    render(<Spinner variant="inline" size={10} className="mt-0.5" />)
    const el = screen.getByRole('status')
    expect(el).toHaveClass('mt-0.5', 'border-[var(--accent)]', 'shrink-0')
    expect(el.style.width).toBe('10px')
  })
})

describe('Skeleton', () => {
  it('按 lines 出行数，末行收窄，整体对读屏隐藏', () => {
    const { container } = render(<Skeleton lines={4} />)
    const wrap = container.firstElementChild as HTMLElement
    expect(wrap).toHaveAttribute('aria-hidden', 'true')
    const rows = Array.from(wrap.children)
    expect(rows).toHaveLength(4)
    expect(rows.every((r) => r.className.includes('shimmer'))).toBe(true)
    expect(rows[0].getAttribute('style')).toContain('width: 100%')
    expect(rows[3].getAttribute('style')).toContain('width: 62%')
  })
})
