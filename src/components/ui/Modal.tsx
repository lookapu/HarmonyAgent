/**
 * 模态：全库 18 处手写模态的统一外壳
 *
 * 阻塞语义由 onClose 是否传入决定，这是刻意的单一开关：
 *   传 onClose  → 可关闭：注册 Esc、遮罩可点、渲染右上角 X
 *   不传 onClose → 阻塞式：三者全部没有（工具权限审核 / ask_user 就是这种，
 *                  用户必须做出选择，不能被 Esc 或点空白绕过）
 *
 * 一律 createPortal 到 body：模态若留在原位置，任何带 transform 的祖先
 * （例如 .render-tier-high .chat-scroll > *）都会让 position:fixed 相对该祖先
 * 定位而不是视口，遮罩就盖不满屏。messageBlocks 的独立审核窗口此前正是为此
 * 手写 portal。
 *
 * 遮罩点击用 onMouseDown 而非 onClick，且比对 e.target === e.currentTarget：
 * 在模态内按下鼠标、拖到遮罩上松开不应关闭（onClick 会误触发）。
 *
 * 遮罩本体复用 index.css 的 .modal-backdrop（已含平涂压暗 + fade-in +
 * z-index: var(--app-z-modal)），本文件只补 flex 居中与内边距。
 */

import { useId } from 'react'
import type { ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { useEscapeKey } from '../../hooks/useEscapeKey'
import { cn } from '../../utils/cn'
import { IconButton } from './IconButton'
import Icon from '../../icons/Icon'
import type { IconName } from '../../icons/Icon'

const sizeCls = {
  sm: 'w-[420px]',
  md: 'w-[480px]',
  lg: 'w-[560px]',
  xl: 'w-[640px]',
  '2xl': 'w-[820px]',
} as const

const maxHeightCls = {
  none: '',
  '80vh': 'max-h-[80vh]',
  '86vh': 'max-h-[86vh]',
} as const

export interface ModalProps {
  open: boolean
  /** 省略即阻塞式：不注册 Esc、遮罩不可点、不渲染关闭按钮 */
  onClose?: () => void
  title?: ReactNode
  icon?: IconName
  size?: keyof typeof sizeCls
  /** light：标题与正文同层无分隔线（确认框、小表单）
   *  formal：标题行带下边框与浅色底（多区块的设置类弹窗） */
  header?: 'light' | 'formal'
  /** top = 顶部 12vh 偏移，⌘K 那类「靠近触发点」的浮层用 */
  align?: 'center' | 'top'
  footer?: ReactNode
  maxHeight?: keyof typeof maxHeightCls
  className?: string
  children?: ReactNode
}

export function Modal({
  open,
  onClose,
  title,
  icon,
  size = 'md',
  header = 'light',
  align = 'center',
  footer,
  maxHeight = '86vh',
  className,
  children,
}: ModalProps) {
  const titleId = useId()
  const { t } = useTranslation()
  useEscapeKey(onClose ?? null, { enabled: open })

  if (!open) return null

  const showHeader = title != null || onClose != null

  return createPortal(
    <div
      className={cn(
        'modal-backdrop flex justify-center p-4',
        align === 'top' ? 'items-start pt-[12vh]' : 'items-center',
      )}
      onMouseDown={(e) => {
        if (onClose && e.target === e.currentTarget) onClose()
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={title != null ? titleId : undefined}
        className={cn(
          'glass-card flex flex-col rounded-xl animate-modal-in max-w-[92vw]',
          sizeCls[size],
          maxHeightCls[maxHeight],
          className,
        )}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {showHeader && (
          <div
            className={cn(
              'flex shrink-0 items-center gap-2',
              header === 'formal'
                ? 'border-b border-[var(--border)] bg-[var(--bg-card)] px-4 py-2.5'
                : 'px-4 pt-3.5 pb-1',
            )}
          >
            {icon && (
              <span aria-hidden="true" className="inline-flex shrink-0">
                <Icon name={icon} size={15} />
              </span>
            )}
            {title != null && (
              <h2
                id={titleId}
                className="min-w-0 flex-1 truncate text-[length:var(--app-text-md)] font-semibold"
              >
                {title}
              </h2>
            )}
            {onClose && (
              <IconButton
                icon="close"
                label={t('common.close')}
                pad="xs"
                iconSize={12}
                onClick={onClose}
                className={cn(title == null && 'ml-auto')}
              />
            )}
          </div>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">{children}</div>

        {footer && (
          <div className="flex shrink-0 items-center justify-end gap-2 px-4 py-3">{footer}</div>
        )}
      </div>
    </div>,
    document.body,
  )
}
