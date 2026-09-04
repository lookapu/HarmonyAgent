/**
 * 空态：无数据 / 无搜索结果 / 未选择 的统一呈现
 *
 * compact 用于面板内联空态（此前全库有 8 处手写单行「暂无…」），
 * 非 compact 用于整页/整栏空态。图标放在 1px 描边的方块里而不是彩色圆盘——
 * IDE 的空态是信息缺失的说明，不是营销插画。
 */

import type { ReactNode } from 'react'
import Icon from '../../icons/Icon'
import type { IconName } from '../../icons/Icon'
import { cn } from '../../utils/cn'

export interface EmptyStateProps {
  icon?: IconName
  title: ReactNode
  description?: ReactNode
  /** 主操作区：传 <Button> 即可 */
  action?: ReactNode
  compact?: boolean
  className?: string
}

export function EmptyState({
  icon,
  title,
  description,
  action,
  compact = false,
  className,
}: EmptyStateProps) {
  if (compact) {
    return (
      <div
        className={cn(
          'flex items-center gap-2 px-3 py-2.5 text-[length:var(--app-text-sm)] text-[var(--text-muted)]',
          className,
        )}
      >
        {icon && (
          <span aria-hidden="true" className="inline-flex shrink-0 opacity-70">
            <Icon name={icon} size={13} />
          </span>
        )}
        <span className="min-w-0 flex-1 truncate">{title}</span>
        {action}
      </div>
    )
  }

  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center gap-2 px-6 py-10 text-center',
        className,
      )}
    >
      {icon && (
        <span
          aria-hidden="true"
          className="mb-1 inline-flex h-9 w-9 items-center justify-center rounded-md border border-[var(--border)] bg-[var(--bg-card)] opacity-80"
        >
          <Icon name={icon} size={17} />
        </span>
      )}
      <div className="text-[length:var(--app-text-md)] font-semibold text-[var(--text-secondary)]">
        {title}
      </div>
      {description && (
        <div className="max-w-[46ch] text-[length:var(--app-text-sm)] leading-relaxed text-[var(--text-muted)]">
          {description}
        </div>
      )}
      {action && <div className="mt-2 flex items-center gap-2">{action}</div>}
    </div>
  )
}
