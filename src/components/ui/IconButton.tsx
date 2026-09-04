/**
 * 图标按钮：全库 40 处手写图标按钮的统一实现
 *
 * label 必填 → 强制生成 aria-label。此前那 40 处多数只有 title，读屏软件读到的是
 * Icon 内部 <img alt="close"> 这类原始图标名，且鼠标之外的用户拿不到任何说明。
 *
 * hoverTone 只提供**底色**反馈，不提供文字色：Icon 是 <img> + filter 实现，
 * 只有「原色 / 反白」两种状态，无法跟随 tone 任意着色（见计划里的 Icon 批次说明）。
 * 所以这里不写 hover:text-* —— 按钮内没有文字，写了是死类。
 */

import type { ButtonHTMLAttributes } from 'react'
import Icon from '../../icons/Icon'
import type { IconName } from '../../icons/Icon'
import { cn } from '../../utils/cn'

const hoverCls = {
  neutral: 'hover:bg-[var(--bg-hover)]',
  accent: 'hover:bg-[var(--accent-100)]',
  danger: 'hover:bg-[var(--danger-50)]',
} as const

const padCls = {
  xs: 'p-0.5',
  sm: 'p-1',
  md: 'p-1.5',
} as const

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  icon: IconName
  /** 无障碍标签，同时用作 title 提示 */
  label: string
  hoverTone?: keyof typeof hoverCls
  pad?: keyof typeof padCls
  /** 带边框方盒（模态头部、工具栏成组按钮用），默认裸按钮 */
  box?: boolean
  iconSize?: number
}

export function IconButton({
  icon,
  label,
  hoverTone = 'neutral',
  pad = 'sm',
  box = false,
  iconSize = 14,
  type = 'button',
  className,
  ...rest
}: IconButtonProps) {
  return (
    <button
      {...rest}
      type={type}
      aria-label={label}
      title={label}
      className={cn(
        'inline-flex shrink-0 items-center justify-center rounded-md text-[var(--text-muted)] disabled:cursor-not-allowed disabled:opacity-45',
        box
          ? 'border border-[var(--border)] bg-[var(--bg-card)]'
          : 'border border-transparent bg-transparent',
        padCls[pad],
        hoverCls[hoverTone],
        className,
      )}
    >
      {/* aria-hidden：Icon 的 <img alt> 会重复读屏内容，按钮的 aria-label 已经说明用途 */}
      <span aria-hidden="true" className="inline-flex">
        <Icon name={icon} size={iconSize} />
      </span>
    </button>
  )
}
