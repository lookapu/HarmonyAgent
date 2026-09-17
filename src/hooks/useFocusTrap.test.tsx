import { describe, it, expect } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { useFocusTrap } from './useFocusTrap'

function Trap({
  enabled,
  moveFocus,
  children,
}: {
  enabled?: boolean
  moveFocus?: boolean
  children?: ReactNode
}) {
  const ref = useRef<HTMLDivElement>(null)
  useFocusTrap(ref, { enabled, moveFocus })
  return (
    <div ref={ref} tabIndex={-1} data-testid="trap">
      {children}
    </div>
  )
}

const tab = (shiftKey = false): void => {
  fireEvent.keyDown(document, { key: 'Tab', shiftKey })
}

describe('useFocusTrap', () => {
  it('挂载时把焦点移进容器的首个可聚焦元素', () => {
    render(
      <Trap>
        <button>a</button>
        <button>b</button>
      </Trap>,
    )
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'a' }))
  })

  it('Tab 到末尾回卷到首个，Shift+Tab 到首个回卷到末尾', () => {
    render(
      <Trap>
        <button>a</button>
        <button>b</button>
      </Trap>,
    )
    const a = screen.getByRole('button', { name: 'a' })
    const b = screen.getByRole('button', { name: 'b' })

    b.focus()
    tab()
    expect(document.activeElement).toBe(a)

    a.focus()
    tab(true)
    expect(document.activeElement).toBe(b)
  })

  it('焦点在容器外时被拉回容器，不会放它留在背后内容上', () => {
    render(
      <>
        <button>outside</button>
        <Trap>
          <button>inner</button>
        </Trap>
      </>,
    )
    const outside = screen.getByRole('button', { name: 'outside' })
    outside.focus()
    expect(document.activeElement).toBe(outside)

    tab()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'inner' }))
  })

  it('卸载时把焦点还给打开前的触发元素', () => {
    function Host() {
      const [open, setOpen] = useState(false)
      return (
        <>
          <button onClick={() => setOpen((v) => !v)}>opener</button>
          {open && (
            <Trap>
              <button>inner</button>
            </Trap>
          )}
        </>
      )
    }
    render(<Host />)
    const opener = screen.getByRole('button', { name: 'opener' })
    opener.focus()

    fireEvent.click(opener)
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'inner' }))

    fireEvent.click(opener)
    expect(screen.queryByRole('button', { name: 'inner' })).toBeNull()
    expect(document.activeElement).toBe(opener)
  })

  // 调用方自己有刻意的 autofocus（打开即聚焦搜索框）时，陷阱不能抢走焦点，
  // 但 Tab 循环仍要拦住
  it('moveFocus=false 不抢焦点，Tab 拦截照常生效', () => {
    render(
      <Trap moveFocus={false}>
        <button>a</button>
        <button>b</button>
      </Trap>,
    )
    const a = screen.getByRole('button', { name: 'a' })
    const b = screen.getByRole('button', { name: 'b' })
    expect(document.activeElement).not.toBe(a)

    b.focus()
    tab()
    expect(document.activeElement).toBe(a)
  })

  // 容器需要 tabindex="-1" 才兜得住：这是 Modal 给 dialog 加它的原因
  it('没有可聚焦子元素时焦点扣在容器上', () => {
    render(<Trap>纯文本</Trap>)
    const root = screen.getByTestId('trap')
    expect(document.activeElement).toBe(root)

    tab()
    expect(document.activeElement).toBe(root)
  })

  it('enabled=false 时完全不介入', () => {
    render(
      <Trap enabled={false}>
        <button>a</button>
      </Trap>,
    )
    expect(document.activeElement).not.toBe(screen.getByRole('button', { name: 'a' }))
  })

  it('非 Tab 键不拦截，Esc 留给 useEscapeKey', () => {
    render(
      <Trap>
        <button>a</button>
        <button>b</button>
      </Trap>,
    )
    const b = screen.getByRole('button', { name: 'b' })
    b.focus()
    fireEvent.keyDown(document, { key: 'Escape' })
    fireEvent.keyDown(document, { key: 'Enter' })
    expect(document.activeElement).toBe(b)
  })
})
