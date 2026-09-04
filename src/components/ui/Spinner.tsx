/**
 * 加载态原子：Spinner（进行中）与 Skeleton（内容占位）
 *
 * 两个 variant 对应两种**角色**，不是同一个东西的两种写法：
 *
 * - `ring`（默认）：面板 / 整页加载。2px 圆环 + 灰轨 + accent 顶边，尺寸 24px 起。
 * - `inline`：行内「这一步正在跑」。1px 圆环 + accent 轨 + 透明顶边，常用 10px，
 *   与同行的 11px 图标对齐。这是全库真正的**主流**写法——toolRuns(172/271)、
 *   plan(81/598/624)、panels(117)、messageBlocks(318/471)、FileTreePanel(436/645)、
 *   GitPanel(312) 共 12+ 处逐字节相同。
 *
 * 原先只有 ring 一种。后果是：上面那 12+ 处 inline 站点一旦迁过来，就会被悄悄
 * 从「accent 轨 + 透明顶」改成「灰轨 + accent 顶」——主色从 90% 的环变成 10% 的环，
 * 是实打实的视觉回归。variant 就是为了不让「收敛」变成「改色」。
 *
 * **不收敛「图标自转」写法**（`<Icon name="refresh" className="animate-spin">`，全库
 * 14 处：Home 4772/6286、CostPage ×3、ProvidersPage、devicePanels ×8）。那是「动作
 * 自己的图标在转」，与「内容在加载」是两个角色；换成 ring 会让按钮在忙时丢掉语义
 * 图标，只剩一个圈。计划里把它算作第 4 种 spinner 写法，实测后认为不该合并。
 *
 * `invert` 对应 Icon 的 `white`：实心底色（primary / armed 按钮）上必须换成白色系，
 * 否则 accent 顶边落在 accent-600 底上等于隐形。
 *
 * Skeleton 复用 index.css 既有的 .shimmer（此前只有 devicePanels 8 处在用），
 * 不新造动画——那条 .shimmer::after 已经有 .render-tier-low 的降速覆盖。
 */

import { cn } from '../../utils/cn'

const variantCls = {
  ring: 'border-2 border-[var(--border-strong)] border-t-[var(--accent)]',
  inline: 'border border-[var(--accent)] border-t-transparent',
} as const

const invertedCls = {
  ring: 'border-2 border-white/40 border-t-white',
  inline: 'border border-white/70 border-t-transparent',
} as const

interface SpinnerProps {
  /** 直径 px，默认 14（对齐 md 号按钮的内容高度）；inline 常用 10 */
  size?: number
  /** ring = 面板/整页加载，inline = 行内单步进行中 */
  variant?: keyof typeof variantCls
  /** 实心底色上用白色系，语义同 Icon 的 white */
  invert?: boolean
  /** 无障碍标签；不传则只暴露 role="status"，由调用方的可见文案说明上下文 */
  label?: string
  className?: string
}

export function Spinner({
  size = 14,
  variant = 'ring',
  invert = false,
  label,
  className,
}: SpinnerProps) {
  return (
    <span
      role="status"
      aria-label={label}
      className={cn(
        'inline-block shrink-0 rounded-full animate-spin',
        invert ? invertedCls[variant] : variantCls[variant],
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
