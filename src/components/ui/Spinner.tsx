/**
 * 加载态原子：Spinner（进行中）与 Skeleton（内容占位）
 *
 * Spinner 收敛此前全库 4 种写法（border-2 / border-t-2 / animate-spin / 自绘 conic
 * 渐变）为一种：2px 圆环 + accent 顶边 + animate-spin。
 *
 * Skeleton 复用 index.css 既有的 .shimmer（此前只有 devicePanels 8 处在用），
 * 不新造动画——那条 .shimmer::after 已经有 .render-tier-low 的降速覆盖。
 */

import { cn } from '../../utils/cn'

interface SpinnerProps {
  /** 直径 px，默认 14（对齐 md 号按钮的内容高度） */
  size?: number
  /** 无障碍标签；不传则只暴露 role="status"，由调用方的可见文案说明上下文 */
  label?: string
  className?: string
}

export function Spinner({ size = 14, label, className }: SpinnerProps) {
  return (
    <span
      role="status"
      aria-label={label}
      className={cn(
        'inline-block shrink-0 rounded-full border-2 border-[var(--border-strong)] border-t-[var(--accent)] animate-spin',
        className,
      )}
      style={{ width: size, height: size }}
    />
  )
}

interface SkeletonProps {
  /** 占位行数，默认 3 */
  lines?: number
  className?: string
}

export function Skeleton({ lines = 3, className }: SkeletonProps) {
  return (
    <div aria-hidden="true" className={cn('space-y-2', className)}>
      {Array.from({ length: lines }, (_, i) => (
        <div
          key={i}
          className="shimmer h-3 rounded-sm bg-[var(--bg-hover)]"
          style={{ width: i === lines - 1 ? '62%' : '100%' }}
        />
      ))}
    </div>
  )
}
