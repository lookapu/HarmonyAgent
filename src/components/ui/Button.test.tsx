import { describe, it, expect, vi, afterEach } from 'vitest'
import { render, screen, fireEvent, act } from '@testing-library/react'
import { useState } from 'react'
import { Button } from './Button'

describe('Button', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('默认 type=button，不会在将来的表单里意外提交', () => {
    render(<Button>ok</Button>)
    expect(screen.getByRole('button')).toHaveAttribute('type', 'button')
  })

  it('variant 映射到既有 CSS 类（primary → .btn-primary）', () => {
    render(<Button variant="primary">go</Button>)
    expect(screen.getByRole('button')).toHaveClass('btn-primary')
  })

  // index.css 只给 .btn-primary:disabled 配了 opacity，其余 variant 得靠基类自己兜
  it('disabled 时每个 variant 都有可见的禁用态，不只靠 cursor', () => {
    render(
      <Button variant="danger" disabled>
        clear
      </Button>,
    )
    const btn = screen.getByRole('button')
    expect(btn).toBeDisabled()
    expect(btn).toHaveClass('disabled:opacity-45')
  })

  it('loading 时禁用、标记 aria-busy，且不触发 onClick', () => {
    const onClick = vi.fn()
    render(
      <Button loading onClick={onClick}>
        save
      </Button>,
    )
    const btn = screen.getByRole('button')
    expect(btn).toBeDisabled()
    expect(btn).toHaveAttribute('aria-busy', 'true')
    fireEvent.click(btn)
    expect(onClick).not.toHaveBeenCalled()
  })

  it('confirm：首次点击进入 armed 态而不执行，再次点击才执行', () => {
    const onClick = vi.fn()
    render(
      <Button variant="danger" confirm onClick={onClick} confirmLabel="确认删除">
        删除
      </Button>,
    )
    const btn = screen.getByRole('button')
    expect(btn).toHaveTextContent('删除')

    fireEvent.click(btn)
    expect(onClick).not.toHaveBeenCalled()
    expect(btn).toHaveTextContent('确认删除')

    fireEvent.click(btn)
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it('armed 态 2.5s 未确认自动解除', () => {
    vi.useFakeTimers()
    render(
      <Button confirm confirmLabel="sure?">
        del
      </Button>,
    )
    const btn = screen.getByRole('button')
    fireEvent.click(btn)
    expect(btn).toHaveTextContent('sure?')

    act(() => {
      vi.advanceTimersByTime(2600)
    })
    expect(btn).toHaveTextContent('del')
  })

  it('受控 armed：交给 onArm，父级决定谁处于 armed', () => {
    const onArm = vi.fn()
    const onClick = vi.fn()
    function List() {
      const [armedId, setArmedId] = useState<string | null>(null)
      return (
        <>
          {['a', 'b'].map((id) => (
            <Button
              key={id}
              confirm
              armed={armedId === id}
              onArm={() => {
                onArm(id)
                setArmedId(id)
              }}
              onClick={() => onClick(id)}
            >
              {id}
            </Button>
          ))}
        </>
      )
    }
    render(<List />)
    const [a, b] = screen.getAllByRole('button')

    // 不变量：只有「当前处于 armed 的那一个」点击才执行 onClick；
    // 点其它 confirm 按钮一律是抢占 armed，不执行。
    fireEvent.click(a) // 无人 armed → a 抢占
    expect(onArm).toHaveBeenCalledWith('a')
    expect(onClick).not.toHaveBeenCalled()

    fireEvent.click(b) // armed 的是 a → b 抢占，a 被解除
    expect(onArm).toHaveBeenCalledWith('b')
    expect(onClick).not.toHaveBeenCalled()

    fireEvent.click(a) // armed 的是 b → a 抢回来
    expect(onArm).toHaveBeenCalledTimes(3)
    expect(onClick).not.toHaveBeenCalled()

    fireEvent.click(b) // armed 的是 a → b 抢占，a 被解除
    expect(onClick).not.toHaveBeenCalled()

    fireEvent.click(b) // b 连续第二次点击且自身 armed → 执行
    expect(onClick).toHaveBeenCalledWith('b')
    expect(onClick).toHaveBeenCalledTimes(1)
  })
})
