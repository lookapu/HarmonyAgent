/**
 * 焦点陷阱：把 Tab 循环关在容器里，挂载时移入焦点、卸载时还给触发元素
 *
 * 存在的理由是 `aria-modal="true"` 不能是空头承诺。这个属性向读屏声明「背后的内容
 * 已惰性、不可达」，可如果焦点还能 Tab 出去，AT 用户就会被带进一片**被声明为不可达、
 * 却真的能聚焦**的内容里——比干脆不写这个属性更糟。ui/Modal 此前正是这个状态，
 * dialogs.tsx 里 6 个已迁移的对话框一起继承了它。
 *
 * 只管 Tab。Esc 交给 useEscapeKey：两个关注点分开，各自的栈语义互不干扰。
 *
 * 不拦截容器外的指针事件——遮罩点击关闭由各模态自己决定（Modal 的 backdropClose）。
 *
 * 不做可见性过滤（offsetParent / getClientRects）：jsdom 不做布局，两者恒为
 * null / 0，会把所有候选都筛掉。选择器本身已经排除 :disabled 与 tabindex="-1"。
 */

import { useEffect } from 'react'
import type { RefObject } from 'react'

const FOCUSABLE =
  'a[href],button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),[tabindex]:not([tabindex="-1"])'

function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE))
}

export interface FocusTrapOptions {
  /** false = 完全不介入（容器尚未挂载时用） */
  enabled?: boolean
  /** 挂载时是否把焦点移进容器，默认 true。调用方自己有刻意 autofocus
   *  （例如打开即聚焦搜索框）时传 false，否则会被这里的「首个可聚焦元素」抢走 */
  moveFocus?: boolean
  /** 卸载时是否把焦点还给打开前的元素，默认 true */
  restore?: boolean
}

export function useFocusTrap(
  ref: RefObject<HTMLElement | null>,
  opts?: FocusTrapOptions,
): void {
  const enabled = opts?.enabled !== false
  const moveFocus = opts?.moveFocus !== false
  const restore = opts?.restore !== false

  useEffect(() => {
    if (!enabled) return
    const root = ref.current
    if (!root) return

    const previous = document.activeElement as HTMLElement | null
    if (moveFocus) {
      // 容器需要 tabindex="-1" 才能在没有任何可聚焦子元素时兜住焦点
      const first = focusables(root)[0]
      if (first) first.focus()
      else root.focus()
    }

    const onKeyDown = (e: KeyboardEvent): void => {
      if (e.key !== 'Tab') return
      const items = focusables(root)
      if (items.length === 0) {
        // 没有可聚焦子元素：把焦点扣在容器上，别让它跑出去
        e.preventDefault()
        root.focus()
        return
      }
      const active = document.activeElement
      const first = items[0]
      const last = items[items.length - 1]
      const outside = active == null || !root.contains(active)
      if (e.shiftKey) {
        if (outside || active === first) {
          e.preventDefault()
          last.focus()
        }
      } else if (outside || active === last) {
        e.preventDefault()
        first.focus()
      }
    }

    // 捕获阶段：容器内若有 stopPropagation 的键盘处理也拦得住
    document.addEventListener('keydown', onKeyDown, true)
    return () => {
      document.removeEventListener('keydown', onKeyDown, true)
      if (restore && previous != null && previous.isConnected) previous.focus()
    }
  }, [enabled, moveFocus, restore, ref])
}
