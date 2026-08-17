import { useEffect, useState, useRef } from 'react'
import {
  subscribePerfRecords,
  clearPerfHistory,
  enableLongTaskMonitor,
  enableLongAnimationFrameMonitor,
  enableFpsMonitor,
  disableFpsMonitor,
  type PerfRecord,
  type PerfRecordKind,
} from '../utils/perfTrace'
import Icon from '../icons/Icon'

/**
 * 性能监控浮窗：右下角悬浮按钮，点击展开显示最近的性能追踪记录。
 * - 默认收起为带颜色圆点的小按钮（最近一次耗时决定颜色）
 * - 展开后显示最近 80 条记录：手动埋点 + 自动检测的长任务(jank) + 低FPS + 启动阶段
 * - 支持清空记录、关闭面板
 * - Ctrl+Shift+P 快捷键切换
 */

const KIND_META: Record<PerfRecordKind, { badge: string; badgeColor: string; barMax: number; unit: string }> = {
  trace:     { badge: 'trace',  badgeColor: 'var(--accent)',         barMax: 500, unit: 'ms' },
  bootstrap: { badge: 'boot',   badgeColor: '#8b5cf6',              barMax: 3000, unit: 'ms' },
  longtask:  { badge: 'jank',   badgeColor: '#ef4444',              barMax: 300, unit: 'ms' },
  lowfps:    { badge: 'FPS',    badgeColor: '#f59e0b',              barMax: 60,  unit: 'fps' },
}

export default function PerfMonitor() {
  const [records, setRecords] = useState<PerfRecord[]>([])
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const [enabled, setEnabled] = useState(() => {
    try {
      return localStorage.getItem('deveco-switch:perf-monitor') === '1'
    } catch {
      return false
    }
  })
  const panelRef = useRef<HTMLDivElement>(null)

  // 订阅记录 + 启用自动监控
  useEffect(() => {
    if (!enabled) return
    const unsub = subscribePerfRecords((recs) => {
      setRecords(recs.slice().reverse())
    })
    enableLongTaskMonitor()
    enableLongAnimationFrameMonitor()
    enableFpsMonitor()
    return () => {
      unsub()
      disableFpsMonitor()
    }
  }, [enabled])

  // 快捷键
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'p') {
        e.preventDefault()
        setEnabled((v) => {
          const next = !v
          try { localStorage.setItem('deveco-switch:perf-monitor', next ? '1' : '0') } catch { /* ignore */ }
          if (next) setOpen(true)
          return next
        })
      }
      if (e.key === 'Escape' && open) setOpen(false)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open])

  // 点击外部关闭
  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [open])

  if (!enabled) {
    return (
      <button
        onClick={() => {
          setEnabled(true)
          setOpen(true)
          try { localStorage.setItem('deveco-switch:perf-monitor', '1') } catch { /* ignore */ }
        }}
        title="性能监控 (Ctrl+Shift+P)"
        className="fixed bottom-2 right-2 z-[9998] w-5 h-5 rounded-full flex items-center justify-center text-[var(--text-muted)]/40 hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
      >
        <Icon name="spark" size={12} />
      </button>
    )
  }

  // 统计信息
  const badCount = records.filter((r) => r.level === 'bad').length
  const warnCount = records.filter((r) => r.level === 'warn').length
  const latest = records[0]
  const btnColor =
    !latest ? 'var(--text-muted)' :
    latest.level === 'bad' ? '#ef4444' :
    latest.level === 'warn' ? '#f59e0b' : '#22c55e'

  const handleCopy = async () => {
    const lines: string[] = []
    lines.push('📊 Deveco Switch 性能报告')
    lines.push(`🕐 ${new Date().toLocaleString()}`)
    lines.push(`📈 共 ${records.length} 条记录 | 🔴 ${badCount} 卡 | 🟡 ${warnCount} 慢`)
    lines.push('─'.repeat(50))
    for (const r of records.slice(0, 30)) {
      const meta = KIND_META[r.kind]
      const icon = r.level === 'bad' ? '🔴' : r.level === 'warn' ? '🟡' : '🟢'
      const tag = `[${meta.badge}]`
      const val = r.kind === 'lowfps' ? `${Math.round(r.totalMs)}fps` : `${r.totalMs.toFixed(0)}ms`
      const time = new Date(r.wallTime).toLocaleTimeString(undefined, { hour12: false })
      lines.push(`${icon} ${tag.padEnd(7)} ${val.padStart(7)}  ${r.label}  (${time})`)
      if (r.meta && Object.keys(r.meta).length > 0) {
        const metaStr = Object.entries(r.meta).map(([k, v]) => `${k}=${v}`).join(' ')
        lines.push(`        ↳ ${metaStr}`)
      }
      if (r.segments.length > 1 || (r.segments.length === 1 && r.segments[0].name !== 'rest' && r.segments[0].name !== 'blocked' && r.segments[0].name !== 'since-start' && r.segments[0].name !== 'fps')) {
        for (const s of r.segments) {
          const pct = r.totalMs > 0 ? Math.round((s.durationMs / r.totalMs) * 100) : 0
          const unit = r.kind === 'lowfps' ? 'fps' : 'ms'
          lines.push(`        · ${s.name.padEnd(20)} ${(s.durationMs.toFixed(0) + unit).padStart(7)}  ${pct}%`)
        }
      }
    }
    if (records.length > 30) {
      lines.push('─'.repeat(50))
      lines.push(`... 还有 ${records.length - 30} 条记录`)
    }
    const text = lines.join('\n')
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // fallback
      const ta = document.createElement('textarea')
      ta.value = text
      ta.style.position = 'fixed'
      ta.style.opacity = '0'
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    }
  }

  return (
    <div className="fixed bottom-3 right-3 z-[9999]" ref={panelRef}>
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-8 h-8 rounded-full flex items-center justify-center shadow-lg border border-[var(--border)] bg-[var(--bg-elevated)] hover:bg-[var(--bg-hover)] transition-all"
        title="性能监控 (Ctrl+Shift+P)"
      >
        <span
          className="w-2.5 h-2.5 rounded-full"
          style={{ backgroundColor: btnColor, boxShadow: `0 0 6px ${btnColor}80` }}
        />
      </button>

      {open && (
        <div
          className="absolute bottom-10 right-0 w-[400px] max-h-[520px] rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)] shadow-2xl flex flex-col overflow-hidden"
          style={{ backdropFilter: 'blur(12px)' }}
        >
          {/* 头部 */}
          <div className="flex items-center gap-2 px-3 py-2 border-b border-[var(--border)]">
            <Icon name="spark" size={13} className="text-[var(--accent)]" />
            <span className="text-[12px] font-medium">性能监控</span>
            <span className="text-[10px] text-[var(--text-muted)] ml-1">{records.length} 条</span>
            {badCount > 0 && <span className="text-[9.5px] font-mono px-1.5 py-0.5 rounded bg-[#ef4444]/15 text-[#ef4444]">{badCount} 卡</span>}
            {warnCount > 0 && <span className="text-[9.5px] font-mono px-1.5 py-0.5 rounded bg-[#f59e0b]/15 text-[#f59e0b]">{warnCount} 慢</span>}
            <div className="ml-auto flex items-center gap-1">
              <button
                onClick={handleCopy}
                className="p-1 rounded hover:bg-[var(--bg-hover)] transition-colors relative"
                title="复制性能报告"
                style={{ color: copied ? '#22c55e' : undefined }}
              >
                <Icon name={copied ? "check" : "copy"} size={11} className={copied ? "text-[#22c55e]" : "text-[var(--text-muted)] hover:text-[var(--text-primary)]"} />
              </button>
              <button
                onClick={() => clearPerfHistory()}
                className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors"
                title="清空记录"
              >
                <Icon name="delete" size={11} />
              </button>
              <button
                onClick={() => {
                  setEnabled(false)
                  setOpen(false)
                  try { localStorage.setItem('deveco-switch:perf-monitor', '0') } catch { /* ignore */ }
                }}
                className="p-1 rounded hover:bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors"
                title="关闭面板"
              >
                <Icon name="close" size={11} />
              </button>
            </div>
          </div>

          {/* 类型过滤说明 */}
          <div className="px-3 py-1 border-b border-[var(--border)]/50 flex items-center gap-2 text-[9px] text-[var(--text-muted)]">
            <KindDot kind="trace" label="手动埋点" />
            <KindDot kind="bootstrap" label="启动" />
            <KindDot kind="longtask" label="长任务(jank)" />
            <KindDot kind="lowfps" label="低FPS" />
            <span className="ml-auto">自动监控已启用</span>
          </div>

          {/* 记录列表 */}
          <div className="flex-1 overflow-y-auto overscroll-contain">
            {records.length === 0 ? (
              <div className="p-6 text-center text-[11px] text-[var(--text-muted)]">
                <p>暂无性能记录</p>
                <p className="mt-1 opacity-70">切换项目/会话/发消息后自动记录</p>
              </div>
            ) : (
              <div className="p-2 space-y-1.5">
                {records.map((r) => (
                  <RecordItem key={r.id} record={r} />
                ))}
              </div>
            )}
          </div>

          {/* 底部 */}
          <div className="px-3 py-1.5 border-t border-[var(--border)] text-[9.5px] text-[var(--text-muted)] flex items-center gap-2">
            <span className="inline-flex items-center gap-1"><span className="w-1.5 h-1.5 rounded-full bg-[#22c55e]" />快</span>
            <span className="inline-flex items-center gap-1"><span className="w-1.5 h-1.5 rounded-full bg-[#f59e0b]" />慢</span>
            <span className="inline-flex items-center gap-1"><span className="w-1.5 h-1.5 rounded-full bg-[#ef4444]" />卡</span>
            <span className="ml-auto">Ctrl+Shift+P 切换</span>
          </div>
        </div>
      )}
    </div>
  )
}

function KindDot({ kind, label }: { kind: PerfRecordKind; label: string }) {
  const meta = KIND_META[kind]
  return (
    <span className="inline-flex items-center gap-1">
      <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: meta.badgeColor }} />
      {label}
    </span>
  )
}

function RecordItem({ record }: { record: PerfRecord }) {
  const [expanded, setExpanded] = useState(false)
  const meta = KIND_META[record.kind]
  const color =
    record.level === 'bad' ? '#ef4444' :
    record.level === 'warn' ? '#f59e0b' : '#22c55e'
  const metaStr = record.meta ? Object.entries(record.meta).map(([k, v]) => `${k.slice(0, 8)}=${String(v).slice(0, 10)}`).join(' ') : ''
  const time = new Date(record.wallTime).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
  const maxSeg = Math.max(...record.segments.map((s) => s.durationMs), 1)

  // 低 FPS 记录不显示进度条（数值越小越差）
  const barWidthPct = record.kind === 'lowfps'
    ? Math.min(100, (record.totalMs / 60) * 100)
    : Math.min(100, (record.totalMs / meta.barMax) * 100)
  const valueText = record.kind === 'lowfps'
    ? `${Math.round(record.totalMs)}${meta.unit}`
    : `${record.totalMs.toFixed(0)}${meta.unit}`

  return (
    <div
      className="rounded-lg border border-[var(--border)]/60 bg-[var(--bg-secondary)]/40 hover:bg-[var(--bg-hover)]/40 transition-colors cursor-pointer overflow-hidden"
      onClick={() => setExpanded((v) => !v)}
    >
      <div className="flex items-center gap-2 px-2 py-1.5">
        <span className="shrink-0 text-[8.5px] font-mono font-semibold px-1 py-px rounded" style={{ color: meta.badgeColor, backgroundColor: `${meta.badgeColor}18` }}>
          {meta.badge}
        </span>
        <span className="text-[11px] font-medium truncate flex-1">{record.label}</span>
        {metaStr && <span className="shrink-0 text-[9px] font-mono text-[var(--text-muted)]/70 max-w-[120px] truncate">{metaStr}</span>}
        <span
          className="shrink-0 text-[11px] font-mono font-semibold tabular-nums"
          style={{ color }}
        >
          {valueText}
        </span>
        <span className="shrink-0 text-[9px] text-[var(--text-muted)]/70 tabular-nums w-12 text-right">{time}</span>
      </div>

      <div className="h-0.5 mx-2 mb-1 bg-[var(--bg-hover)] rounded-full overflow-hidden">
        <div
          className="h-full rounded-full transition-all"
          style={{ width: `${barWidthPct}%`, backgroundColor: color }}
        />
      </div>

      {expanded && record.segments.length > 0 && (
        <div className="px-2 pb-2 space-y-0.5">
          {record.segments.map((seg, i) => (
            <div key={i} className="flex items-center gap-2 text-[10px]">
              <span className="text-[var(--text-muted)] w-[100px] text-right truncate shrink-0 font-mono">{seg.name}</span>
              <div className="flex-1 h-2.5 bg-[var(--bg-hover)] rounded overflow-hidden">
                <div
                  className="h-full rounded"
                  style={{
                    width: `${Math.max(2, (seg.durationMs / maxSeg) * 100)}%`,
                    backgroundColor: seg.durationMs > 200 ? '#f59e0b' : seg.durationMs > 50 ? 'var(--accent)' : '#22c55e',
                    opacity: 0.7,
                  }}
                />
              </div>
              <span className="text-[var(--text-secondary)] font-mono tabular-nums w-[52px] text-right shrink-0">
                {record.kind === 'lowfps' ? `${Math.round(seg.durationMs)}fps` : `${seg.durationMs.toFixed(0)}ms`}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
