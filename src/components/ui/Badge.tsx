/**
 * 徽章：状态/计数/标签的最小语义单元
 *
 * shape 默认 square（IDE 向：DevEco / JetBrains 的状态标记是方角小标签），
 * 圆角胶囊只留给 index.css 里既有的 .badge-tone-*（33 处在用，本批不迁）。
 * 语义色一律走 tokens.ts 的 tone 映射，不再各自写 /10 /15 /20 透明度。
 */

import type { ReactNode } from 'react'
import { cn } from '../../utils/cn'
import { toneText, toneWash } from './tokens'
import type { Tone } from './tokens'

const sizeCls = {
  xs: 'h-4 px-1 text-[length:var(--app-text-2xs)]',
  sm: 'h-5 px-1.5 text-[length:var(--app-text-xs)]',
} as const

const shapeCls = {
  square: 'rounded-sm border border-[var(--border)]',
  pill: 'rounded-full',
} as const

export interface BadgeProps {
  tone?: Tone
  size?: keyof typeof sizeCls
  shape?: keyof typeof shapeCls
  /** 等宽字体：版本号、token 名、耗时这类需要纵向对齐的内容 */
  mono?: boolean
  className?: string
  children: ReactNode
}

export function Badge({
  tone = 'neutral',
  size = 'xs',
  shape = 'square',
  mono = false,
  className,
  children,
}: BadgeProps) {
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center gap-1 whitespace-nowrap font-semibold',
        sizeCls[size],
        shapeCls[shape],
        toneWash[tone],
        toneText[tone],
        mono && 'font-mono tnum',
        className,
      )}
    >
      {children}
    </span>
  )
}
