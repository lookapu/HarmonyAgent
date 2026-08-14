import type { RuntimeProgress } from '../api/runtimeProgress'

/** 运行时（Node / Git / JDK）下载/安装进度条：阶段文字 + 百分比 + 实时速度 */
export default function RuntimeProgressBar({ progress }: { progress: RuntimeProgress }) {
  const showBar = progress.phase === 'download' && progress.percent != null
  const speedMb = progress.speed != null ? (progress.speed / 1024 / 1024).toFixed(1) : null
  return (
    <div className="mt-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg px-3 py-2">
      <div className="flex items-center justify-between gap-2 text-xs">
        <span className="text-[var(--text-secondary)]">{progress.message}</span>
        {showBar && (
          <span className="font-mono text-[var(--accent)] shrink-0">
            {progress.percent!.toFixed(1)}%{speedMb != null && ` · ${speedMb} MB/s`}
          </span>
        )}
      </div>
      {showBar && (
        <div className="h-1.5 bg-[var(--bg-secondary)] rounded-full mt-2 overflow-hidden">
          <div
            className="h-full bg-[var(--accent)] rounded-full transition-all duration-300"
            style={{ width: `${Math.min(progress.percent!, 100)}%` }}
          />
        </div>
      )}
    </div>
  )
}
