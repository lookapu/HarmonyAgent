import { describe, it, expect } from 'vitest'
import { render, screen } from '@testing-library/react'
import { Field, TextArea } from './Field'

describe('Field / TextArea', () => {
  it('label 通过 htmlFor 与控件关联（此前手写 label 全是裸的）', () => {
    render(<Field label="标题" />)
    const input = screen.getByRole('textbox')
    expect(input.id).toBeTruthy()
    expect(screen.getByText('标题')).toHaveAttribute('for', input.id)
  })

  it('error 同时改边框、置 aria-invalid、并用 aria-describedby 关联错误文案', () => {
    render(<Field label="标题" error="必填" />)
    const input = screen.getByRole('textbox')
    expect(input).toHaveClass('border-[var(--danger)]')
    expect(input).toHaveAttribute('aria-invalid', 'true')
    expect(input).toHaveAttribute('aria-describedby', expect.stringContaining(input.id))
    expect(screen.getByText('必填').id).toBe(input.getAttribute('aria-describedby'))
  })

  it('hint 与 error 可以同时存在，两者都进 aria-describedby', () => {
    render(<TextArea hint="最多 500 字" error="太长了" />)
    const describedby = screen.getByRole('textbox').getAttribute('aria-describedby') ?? ''
    expect(describedby.split(' ')).toHaveLength(2)
  })

  // rows={3} 在 md 档约 75px，若仍兜 min-h-[80px] 就会被强行撑高，调用方给的 rows 形同虚设
  it('显式给了 rows 就不再兜 min-h，没给才兜', () => {
    const { rerender } = render(<TextArea rows={3} />)
    expect(screen.getByRole('textbox')).not.toHaveClass('min-h-[80px]')
    rerender(<TextArea />)
    expect(screen.getByRole('textbox')).toHaveClass('min-h-[80px]')
  })

  it('mono 走 font-mono，不靠调用方传 className 覆盖', () => {
    render(<Field mono />)
    expect(screen.getByRole('textbox')).toHaveClass('font-mono')
  })

  it('className 落在外层容器而不是控件上（控件尺寸只能走 fieldSize）', () => {
    render(<Field className="mt-4" />)
    expect(screen.getByRole('textbox')).not.toHaveClass('mt-4')
    expect(screen.getByRole('textbox').parentElement).toHaveClass('mt-4')
  })
})
