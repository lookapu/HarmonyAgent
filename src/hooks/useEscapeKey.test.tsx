import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, fireEvent, screen } from '@testing-library/react'
import { useState } from 'react'
import { useEscapeKey } from './useEscapeKey'

const pressEsc = (): void => {
  fireEvent.keyDown(window, { key: 'Escape' })
}

function Probe({
  label,
  onEsc,
  enabled,
}: {
  label: string
  onEsc: (label: string) => void
  enabled?: boolean
}) {
  useEscapeKey(() => onEsc(label), { enabled })
  return <div>{label}</div>
}

function Latest({ onEsc }: { onEsc: (n: number) => void }) {
  const [n, setN] = useState(1)
  useEscapeKey(() => onEsc(n))
  return <button onClick={() => setN(n + 1)}>bump</button>
}

describe('useEscapeKey', () => {
  let calls: string[]
  let onEsc: (label: string) => void

  beforeEach(() => {
    calls = []
    onEsc = (label) => calls.push(label)
  })

  it('Esc 触发已挂载的 handler', () => {
    render(<Probe label="a" onEsc={onEsc} />)
    pressEsc()
    expect(calls).toEqual(['a'])
  })

  it('嵌套时只关最上层，下层保留', () => {
    render(<Probe label="outer" onEsc={onEsc} />)
    const inner = render(<Probe label="inner" onEsc={onEsc} />)
    pressEsc()
    expect(calls).toEqual(['inner'])

    inner.unmount()
    pressEsc()
    expect(calls).toEqual(['inner', 'outer'])
  })

  it('handler 为 null 时不入栈，Esc 落到下一层', () => {
    render(<Probe label="outer" onEsc={onEsc} />)
    render(<Probe label="inert" onEsc={onEsc} enabled={false} />)
    pressEsc()
    expect(calls).toEqual(['outer'])
  })

  it('enabled 切到 false 后不再响应', () => {
    const view = render(<Probe label="a" onEsc={onEsc} enabled />)
    pressEsc()
    expect(calls).toEqual(['a'])

    view.rerender(<Probe label="a" onEsc={onEsc} enabled={false} />)
    pressEsc()
    expect(calls).toEqual(['a'])
  })

  it('非 Escape 键不触发', () => {
    render(<Probe label="a" onEsc={onEsc} />)
    fireEvent.keyDown(window, { key: 'Enter' })
    fireEvent.keyDown(window, { key: 'a' })
    expect(calls).toEqual([])
  })

  it('handler 始终读到最新闭包，无需重新注册', () => {
    const seen: number[] = []
    render(<Latest onEsc={(n) => seen.push(n)} />)
    fireEvent.click(screen.getByRole('button'))
    pressEsc()
    expect(seen).toEqual([2])
  })

  it('栈清空后移除全局监听器，不留泄漏', () => {
    const remove = vi.spyOn(window, 'removeEventListener')
    const view = render(<Probe label="a" onEsc={onEsc} />)
    view.unmount()
    expect(remove).toHaveBeenCalledWith('keydown', expect.any(Function))
    remove.mockRestore()
  })
})
