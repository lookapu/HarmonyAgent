/**
 * Esc 关闭：模块级栈 + 全局单监听
 *
 * 此前 18 处模态各自 window.addEventListener('keydown')，嵌套时一次 Esc 会把
 * 所有层一起关掉。这里改成：挂载即入栈、卸载即出栈，全局只注册一个监听器，
 * Esc 只触发栈顶那一个 handler。
 *
 * 刻意不调用 e.preventDefault()：本项目面向中文输入，Esc 的默认行为之一是
 * 取消 IME 组合输入，拦掉它会让用户没法退出候选词状态。
 *
 * 用法：
 *   useEscapeKey(onClose)                       // 常开
 *   useEscapeKey(open ? onClose : null)         // 条件挂载（null = 不入栈）
 *   useEscapeKey(onClose, { enabled: open })    // 同上，组件常驻时用这个
 *
 * 阻塞式模态（无 onClose）直接不调用本 hook，语义与「不注册 Esc」一致。
 */

import { useEffect, useRef } from 'react'

const stack: Array<() => void> = []
let listening = false

function onKeyDown(e: KeyboardEvent): void {
  if (e.key !== 'Escape') return
  const top = stack[stack.length - 1]
  if (top) top()
}

export function useEscapeKey(
  onEscape: (() => void) | null,
  opts?: { enabled?: boolean },
): void {
  const active = opts?.enabled !== false && onEscape !== null
  const latest = useRef(onEscape)

  useEffect(() => {
    latest.current = onEscape
  }, [onEscape])

  useEffect(() => {
    if (!active) return
    const entry = () => latest.current?.()
    stack.push(entry)
    if (!listening) {
      window.addEventListener('keydown', onKeyDown)
      listening = true
    }
    return () => {
      // 用 indexOf 而非 pop：enabled 切换时清理顺序不保证是 LIFO
      const i = stack.indexOf(entry)
      if (i >= 0) stack.splice(i, 1)
      if (listening && stack.length === 0) {
        window.removeEventListener('keydown', onKeyDown)
        listening = false
      }
    }
  }, [active])
}
