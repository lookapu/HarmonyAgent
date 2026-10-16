/**
 * 性能指标卡：当前值 + SVG 迷你趋势曲线。
 * 原本与三块面板同处 devicePanels.tsx，因 Home 对三块面板各自 lazy()
 * 而本文件不 import 任何东西，拆开后它单独成块。
 */

/** 性能指标卡：当前值 + SVG 迷你趋势曲线（数据点 < 2 时只显示数值） */
export function PerfCard({
  label,
  value,
  unit,
  color,
  data,
}: {
  label: string
  value: number
  unit: string
  color: string
  data: number[]
}) {
  const valid = data.filter((v) => v >= 0)
  const show = value >= 0
  return (
    <div className="rounded-md modern-card border-[var(--border)] px-2 py-1.5">
      <div className="flex items-baseline justify-between gap-1">
        <span className="text-[length:var(--app-text-micro)] text-[var(--text-muted)]">{label}</span>
        <span className="text-[length:var(--app-text-xs)] font-mono font-medium" style={{ color: show ? color : 'var(--text-muted)' }}>
          {show ? `${Math.round(value * 10) / 10}${unit}` : '--'}
        </span>
      </div>
      {valid.length >= 2 ? (
        <svg viewBox="0 0 100 26" preserveAspectRatio="none" className="mt-1 h-6 w-full">
          <polyline
            fill="none"
            stroke={color}
            strokeWidth="1.5"
            strokeLinejoin="round"
            strokeLinecap="round"
            points={valid
              .map((v, i) => {
                const x = (i / Math.max(valid.length - 1, 1)) * 100
                const y = 25 - (Math.max(0, Math.min(v, 100)) / 100) * 24
                return `${x.toFixed(1)},${y.toFixed(1)}`
              })
              .join(' ')}
          />
        </svg>
      ) : (
        <div className="mt-1 h-6" />
      )}
    </div>
  )
}
