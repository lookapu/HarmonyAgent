/**
 * 分段控件：2~4 个互斥选项的单选组
 *
 * 为什么不用原生 <select>：主题/语言这类「随时会切、选项少、且切了立刻见效」的
 * 设置，下拉多一次点击、多一层遮罩，还看不到当前值之外还有哪些选项。
 * 也不再自己写 radio：原生 radio 的视觉要在两套主题下各调一遍，
 * 还得补 role/键盘/焦点环——不如直接用 button[aria-pressed]。
 *
 * 选 button 而非 radio 的代价：没有方向键在选项间移动（radio 原生有）。
 * 但本控件只有 2~4 项、选项都很宽，且 Tab 可达每一项，
 * 实际使用中不构成障碍；换来的是完全可控的视觉与零额外样式成本。
 */

import type { ReactNode } from 'react'
import { cn } from '../../utils/cn'

export interface SegmentedOption<T extends string> {
  value: T
  label: ReactNode
  /** 选项的辅助说明；置位时以 title 呈现，不占版面 */
  title?: string
}

export interface SegmentedProps<T extends string> {
  options: SegmentedOption<T>[]
  value: T
  onChange: (value: T) => void
  /** 无障碍名；未传时退化为 group，不读屏也能靠视觉分辨 */
  label: string
  size?: 'sm' | 'md'
  className?: string
  disabled?: boolean
}

const sizeCls = {
  sm: 'h-7 px-2.5 text-[length:var(--app-text-sm)]',
  md: 'h-8 px-3 text-[length:var(--app-text-md)]',
} as const

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  label,
  size = 'sm',
  className,
  disabled,
}: SegmentedProps<T>) {
  return (
    <div
      role="group"
      aria-label={label}
      className={cn(
        'inline-flex shrink-0 items-center gap-0.5 rounded-md border border-[var(--border)] bg-[var(--bg-primary)] p-0.5',
        className,
      )}
    >
      {options.map((opt) => {
        const active = opt.value === value
        return (
          <button
            key={opt.value}
            type="button"
            // aria-pressed 而非 role="radio"：本控件用 button 实现，
            // 声明成 radio 会让读屏期待方向键行为，而我们没有。
            aria-pressed={active}
            title={opt.title}
            disabled={disabled}
            onClick={() => onChange(opt.value)}
            className={cn(
              'rounded-[4px] font-medium whitespace-nowrap',
              sizeCls[size],
              active
                ? 'bg-[var(--accent-soft)] text-[var(--accent)]'
                : 'text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]',
              disabled && 'cursor-not-allowed opacity-50',
            )}
          >
            {opt.label}
          </button>
        )
      })}
    </div>
  )
}
