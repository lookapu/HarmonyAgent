/**
 * 会话事件时间线（右侧栏 Timeline tab）：
 * 读取 session_events 全部事件，按 trace_id 分组折叠——一次任务（一轮用户消息触发的
 * 完整执行）的全部事件共享同一 trace_id，折叠为一个组；组头展示短 ID、事件数、
 * 工具调用数、失败数与耗时，点击展开/收起。
 * 无 trace_id 的系统/旁路事件归入「其他事件」组并默认展开。
 * 事件行：时间 + 类型徽章 + 摘要（tool_call 显示工具名与参数、tool_result 显示成败、
 * 消息类显示内容截断）；点击行展开完整 payload JSON。
 */
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import { getSessionEvents, type SessionEvent } from '../api/project'

const OTHER = '__other__'

/** 事件类型 → 徽章配色 */
function badgeOf(type: string): string {
  switch (type) {
    case 'user_message': return 'bg-[var(--accent-soft)] text-[var(--accent)]'
    case 'assistant_message': return 'bg-[#e6f4fe] text-[#149eca] dark:bg-[#149eca]/15 dark:text-[#61dafb]'
    case 'tool_call': return 'bg-[#fff3e0] text-[#e76f00] dark:bg-[#e76f00]/15 dark:text-[#fbbf24]'
    case 'tool_result': return 'bg-[#e9f9e3] text-[#5fa04e] dark:bg-[#5fa04e]/15 dark:text-[#86efac]'
    case 'context_compress': return 'bg-[var(--warning)]/15 text-[var(--warning)]'
    default: return 'bg-[var(--bg-hover)] text-[var(--text-secondary)]'
  }
}

/** 事件类型徽章短名（i18n） */
function typeLabel(type: string, t: TFunction): string {
  switch (type) {
    case 'user_message': return t('home.timelineEventUser')
    case 'assistant_message': return t('home.timelineEventAssistant')
    case 'tool_call': return t('home.timelineEventToolCall')
    case 'tool_result': return t('home.timelineEventToolResult')
    case 'system_note': return t('home.timelineEventSystem')
    case 'context_compress': return t('home.timelineEventCompress')
    default: return type
  }
}

/** 事件摘要（单行）：tool_call=工具名+参数；tool_result=成败+输出；消息类=内容截断
 *  返回 ReactNode 便于内嵌 span 上色（工具名加色、user 加粗、result 用绿/红点缀） */
function eventSummary(e: SessionEvent, t: TFunction): ReactNode {
  const p = e.payload
  const cut = (s: string, n = 120) => (s.length > n ? `${s.slice(0, n)}…` : s)
  switch (e.event_type) {
    case 'tool_call': {
      const name = String(p.name ?? '')
      const args = p.args ? JSON.stringify(p.args) : ''
      const s = cut(args)
      if (!s) return <span className="font-mono">{name}</span>
      return (
        <>
          <span className="font-mono text-[var(--accent)]">{name}</span>
          <span className="text-[var(--text-muted)]"> · </span>
          <span className="font-mono">{s}</span>
        </>
      )
    }
    case 'tool_result': {
      const ok = p.ok !== false
      const out = String(p.output ?? '')
      const s = cut(out)
      return (
        <>
          <span className={ok ? 'text-[var(--success)]' : 'text-[var(--danger)]'}>{ok ? '✓' : '✗'}</span>
          <span> {s}</span>
        </>
      )
    }
    case 'user_message': {
      const c = cut(String(p.content ?? ''))
      return <span className="font-medium text-[var(--text-primary)]">{c}</span>
    }
    case 'assistant_message': {
      const c = cut(String(p.content ?? ''))
      return <span>{c}</span>
    }
    case 'system_note': {
      const c = cut(String(p.text ?? ''))
      return <span className="italic text-[var(--text-muted)]">{c}</span>
    }
    case 'context_compress': {
      const trigger = String(p.trigger ?? '')
      const detail =
        p.old_limit != null && p.new_limit != null
          ? t('home.timelineCompressLimit', { old: String(p.old_limit), new: String(p.new_limit) })
          : p.keep != null
            ? t('home.timelineCompressKeep', { keep: String(p.keep) })
            : ''
      return (
        <span className="text-[var(--warning)]">
          {trigger === 'active'
            ? t('home.timelineCompressActive')
            : trigger === 'overflow'
              ? t('home.timelineCompressOverflow')
              : t('home.timelineCompressManual')}
          {detail}
        </span>
      )
    }
    default:
      return <span className="font-mono">{cut(JSON.stringify(p), 120)}</span>
  }
}

/** 按 trace_id 分组（组内按 seq 升序；无 trace_id 归入 OTHER 组） */
function groupByTrace(events: SessionEvent[]): { key: string; trace: string | null; events: SessionEvent[] }[] {
  const map = new Map<string, SessionEvent[]>()
  for (const e of events) {
    const k = e.trace_id ?? OTHER
    const arr = map.get(k)
    if (arr) arr.push(e)
    else map.set(k, [e])
  }
  // 其他事件排最后；有 trace_id 的组按首事件 seq 升序
  const keys = [...map.keys()].sort((a, b) => {
    if (a === OTHER) return 1
    if (b === OTHER) return -1
    return map.get(a)![0].seq - map.get(b)![0].seq
  })
  return keys.map((k) => ({ key: k, trace: k === OTHER ? null : k, events: map.get(k)! }))
}

function fmtTime(ms: number): string {
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

/* ============ [76] 调用链 DAG 视图：tool_call/tool_result 配对建链 ============ */

/** 调用链节点：一次工具调用的完整生命周期（无 call_id，按“最近未配对 call → 下一 result”配对） */
interface ChainNode {
  name: string
  args: string
  /** null=无结果（进行中/中断） */
  ok: boolean | null
  output: string
  startedAt: number
  endedAt: number | null
  /** 重试自哪个节点索引（同工具失败后再次调用；null=首调） */
  retryOf: number | null
  durationMs: number | null
}

function buildChain(events: SessionEvent[]): ChainNode[] {
  const nodes: ChainNode[] = []
  let pending: { name: string; args: string; startedAt: number } | null = null
  for (const e of events) {
    if (e.event_type === 'tool_call') {
      pending = {
        name: String(e.payload.name ?? ''),
        args: e.payload.args ? JSON.stringify(e.payload.args) : '',
        startedAt: e.created_at,
      }
    } else if (e.event_type === 'tool_result' && pending) {
      const ok = e.payload.ok !== false
      const output = String(e.payload.output ?? '')
      const prevIdx = nodes.length - 1
      // 重试检测：紧邻上一节点同名且失败 → 本轮视为重试（连接线用虚线区分）
      const retryOf =
        prevIdx >= 0 && nodes[prevIdx].name === pending.name && nodes[prevIdx].ok === false ? prevIdx : null
      nodes.push({
        name: pending.name,
        args: pending.args,
        ok,
        output: output.length > 90 ? `${output.slice(0, 90)}…` : output,
        startedAt: pending.startedAt,
        endedAt: e.created_at,
        retryOf,
        durationMs: e.created_at > pending.startedAt ? e.created_at - pending.startedAt : null,
      })
      pending = null
    }
  }
  // 未配对的调用（任务中断/结果未落库）：灰色节点占位
  if (pending) {
    nodes.push({
      name: pending.name,
      args: pending.args,
      ok: null,
      output: '',
      startedAt: pending.startedAt,
      endedAt: null,
      retryOf: null,
      durationMs: null,
    })
  }
  return nodes
}

/** DAG 节点配色：成功绿 / 失败红 / 未知灰 */
function nodeTone(ok: boolean | null): { dot: string; border: string; label: string } {
  if (ok === true) return { dot: 'bg-[var(--success)]', border: 'border-[var(--success)]/40', label: 'text-[var(--success)]' }
  if (ok === false) return { dot: 'bg-[var(--danger)]', border: 'border-[var(--danger)]/40', label: 'text-[var(--danger)]' }
  return { dot: 'bg-[var(--text-muted)]', border: 'border-[var(--border)]', label: 'text-[var(--text-muted)]' }
}

export default function TimelinePanel({
  conversationId,
  refreshTick,
}: {
  conversationId: string | null
  /** 外部刷新信号（新事件落库后 +1 触发重载） */
  refreshTick: number
}) {
  const { t } = useTranslation()
  const [events, setEvents] = useState<SessionEvent[] | null>(null)
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set())
  const [expandedIds, setExpandedIds] = useState<Set<number>>(() => new Set())
  const [loading, setLoading] = useState(false)
  // [76] 视图切换：list=事件流水（默认） / dag=工具调用链
  const [view, setView] = useState<'list' | 'dag'>('list')

  // 会话切换 / 外部刷新信号 → 重载事件
  useEffect(() => {
    if (!conversationId) {
      setEvents(null)
      return
    }
    let cancelled = false
    setLoading(true)
    getSessionEvents(conversationId)
      .then((v) => {
        if (cancelled) return
        setEvents(v.events)
        // 默认折叠有 trace_id 的任务组，展开「其他事件」
        setCollapsed(new Set(v.events.map((e) => e.trace_id).filter((x): x is string => x != null)))
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [conversationId, refreshTick])

  const groups = useMemo(() => groupByTrace(events ?? []), [events])
  // [76] 各组的调用链（DAG 视图数据，按组惰性构建）
  const chains = useMemo(() => {
    const map = new Map<string, ChainNode[]>()
    for (const g of groups) map.set(g.key, buildChain(g.events))
    return map
  }, [groups])

  const toggleGroup = (key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const toggleEvent = (id: number) => {
    setExpandedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const setAllCollapsed = (all: boolean) => {
    setCollapsed(all ? new Set(groups.map((g) => g.key)) : new Set())
  }

  const copyTrace = (trace: string) => {
    void navigator.clipboard.writeText(trace)
  }

  if (!conversationId) return null

  return (
    <div className="flex flex-col h-full">
      {/* 工具栏：视图切换 + 展开/折叠全部 + 刷新
          窄栏下 tip 保持完整不省略（不 truncate），外层 overflow-x-auto 兜底横向滚动；
          title 提供完整文本给 hover tooltip。 */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-[var(--border)] shrink-0 overflow-x-auto whitespace-nowrap">
        <span
          className="shrink-0 text-[11px] text-[var(--text-muted)]"
          title={t('home.timelineTip')}
        >
          {t('home.timelineTip')}
        </span>
        <div className="flex items-center gap-1 shrink-0">
          {/* [76] 视图切换：事件流水 / 调用链 DAG */}
          <div className="flex items-center rounded-lg bg-[var(--bg-hover)] p-0.5 mr-1">
            <button
              onClick={() => setView('list')}
              className={`px-2 py-0.5 rounded text-[11px] transition-colors ${view === 'list' ? 'tab-soft' : 'tab-inactive'}`}
            >
              {t('home.timelineListView')}
            </button>
            <button
              onClick={() => setView('dag')}
              className={`px-2 py-0.5 rounded text-[11px] transition-colors ${view === 'dag' ? 'tab-soft' : 'tab-inactive'}`}
            >
              {t('home.timelineDagView')}
            </button>
          </div>
          <button
            onClick={() => setAllCollapsed(false)}
            className="px-2 py-0.5 rounded text-[11px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {t('home.timelineExpandAll')}
          </button>
          <button
            onClick={() => setAllCollapsed(true)}
            className="px-2 py-0.5 rounded text-[11px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {t('home.timelineCollapseAll')}
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-2 space-y-1.5">
        {!events && !loading && (
          <div className="px-3 py-8 text-center text-[12.5px] text-[var(--text-muted)]">{t('home.timelineEmpty')}</div>
        )}
        {loading && (
          <div className="px-3 py-8 text-center text-[12.5px] text-[var(--text-muted)]">…</div>
        )}
        {groups.map((g) => {
          const isOpen = !collapsed.has(g.key)
          const toolCalls = g.events.filter((e) => e.event_type === 'tool_call').length
          const fails = g.events.filter((e) => e.event_type === 'tool_result' && e.payload.ok === false).length
          const first = g.events[0].created_at
          const last = g.events[g.events.length - 1].created_at
          const dur = last > first ? `${((last - first) / 1000).toFixed(1)}s` : ''
          return (
            <div key={g.key} className="rounded-lg modern-card overflow-hidden">
              {/* 组头：左侧 trace 短 ID + 计数（窄栏可省略），右侧耗时 + 复制 */}
              <button
                onClick={() => toggleGroup(g.key)}
                className="w-full flex items-center gap-1.5 px-2.5 py-1.5 text-left hover:bg-[var(--bg-hover)] transition-colors"
              >
                <svg
                  width="10"
                  height="10"
                  viewBox="0 0 10 10"
                  className={`shrink-0 text-[var(--text-muted)] transition-transform ${isOpen ? 'rotate-90' : ''}`}
                >
                  <path d="M2 1l6 4-6 4V1z" fill="currentColor" />
                </svg>
                <span className="shrink-0 font-mono text-[10.5px] text-[var(--accent)]">
                  {g.trace ? `trace:${g.trace.slice(0, 8)}` : t('home.timelineOther')}
                </span>
                <span className="shrink-0 text-[10.5px] text-[var(--text-muted)] tabular-nums">
                  {t('home.timelineEvents', { count: String(g.events.length) })}
                </span>
                {toolCalls > 0 && (
                  <span className="shrink-0 text-[10.5px] text-[var(--text-muted)] tabular-nums">
                    · {t('home.timelineCalls', { count: String(toolCalls) })}
                  </span>
                )}
                {fails > 0 && (
                  <span className="shrink-0 text-[10.5px] text-[var(--danger)] font-medium tabular-nums">
                    · {t('home.timelineFails', { count: String(fails) })}
                  </span>
                )}
                {dur && (
                  <span className="ml-auto shrink-0 text-[10.5px] text-[var(--text-muted)] tabular-nums">{dur}</span>
                )}
                {g.trace && (
                  <span
                    role="button"
                    title={t('home.timelineCopyTrace')}
                    onClick={(e) => {
                      e.stopPropagation()
                      copyTrace(g.trace!)
                    }}
                    className="shrink-0 ml-1 p-0.5 rounded text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-primary)]"
                  >
                    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <rect x="9" y="9" width="13" height="13" rx="2" />
                      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                    </svg>
                  </span>
                )}
              </button>
              {/* 组内内容：list=事件流水 / dag=调用链 */}
              {isOpen && (
                <div className="border-t border-[var(--border)]">
                  {view === 'dag' ? (
                    /* [76] 调用链 DAG：节点纵向排列，实线=顺序执行、虚线=失败重试 */
                    <ChainView nodes={chains.get(g.key) ?? []} />
                  ) : (
                    <>
                      {g.events.map((e) => {
                        const expanded = expandedIds.has(e.id)
                        return (
                          <div key={e.id} className="border-b border-[var(--border)]/60 last:border-b-0">
                            <button
                              onClick={() => toggleEvent(e.id)}
                              className="w-full flex items-start gap-1.5 px-2.5 py-1.5 text-left hover:bg-[var(--bg-hover)] transition-colors"
                            >
                              {/* 时间戳：固定宽度，避免被徽章/内容挤压覆盖 */}
                              <span className="shrink-0 font-mono text-[10px] text-[var(--text-muted)] leading-5 w-[58px] tabular-nums">
                                {fmtTime(e.created_at)}
                              </span>
                              {/* 类型徽章：固定 2 字宽度（用户/助手/工具等），避免被内容撑长 */}
                              <span className={`shrink-0 inline-flex items-center justify-center text-[10px] px-1.5 py-0.5 rounded font-medium leading-4 min-w-[28px] ${badgeOf(e.event_type)}`}>
                                {typeLabel(e.event_type, t)}
                              </span>
                              {/* 摘要：唯一可压缩元素，长内容省略号 */}
                              <span className="min-w-0 flex-1 text-[11.5px] text-[var(--text-secondary)] truncate leading-5">
                                {eventSummary(e, t)}
                              </span>
                            </button>
                            {expanded && (
                              <pre className="mx-3 mb-2 px-2.5 py-1.5 rounded bg-[var(--bg-window)] text-[10.5px] leading-4 text-[var(--text-secondary)] overflow-x-auto whitespace-pre-wrap break-all max-h-56 overflow-y-auto">
                                {JSON.stringify(e.payload, null, 2)}
                              </pre>
                            )}
                          </div>
                        )
                      })}
                    </>
                  )}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

/**
 * [76] 调用链 DAG 视图：节点纵向排列，实线=顺序执行，虚线=失败重试；
 * 失败/未知节点可点击展开输出详情。
 */
function ChainView({ nodes }: { nodes: ChainNode[] }) {
  const { t } = useTranslation()
  const [expandedIdx, setExpandedIdx] = useState<Set<number>>(() => new Set())
  if (nodes.length === 0) {
    return <div className="px-3 py-4 text-center text-[11px] text-[var(--text-muted)]">{t('home.timelineChainEmpty')}</div>
  }
  return (
    <div className="px-3 py-2.5 space-y-0">
      {nodes.map((n, idx) => {
        const tone = nodeTone(n.ok)
        const isRetry = n.retryOf !== null
        const expanded = expandedIdx.has(idx)
        return (
          <div key={idx} className="flex gap-2.5">
            {/* 左侧连接线：上一节点指向本节点（重试用虚线） */}
            <div className="flex flex-col items-center shrink-0">
              {idx > 0 && <div className={`w-px h-2.5 ${isRetry ? 'border-l border-dashed border-[var(--warning)]' : 'bg-[var(--border)]'}`} />}
              <div className={`w-2 h-2 rounded-full ${tone.dot} shrink-0 mt-0.5`} />
              {idx < nodes.length - 1 && <div className={`w-px flex-1 min-h-3 ${nodes[idx + 1].retryOf === idx ? 'border-l border-dashed border-[var(--warning)]' : 'bg-[var(--border)]'}`} />}
            </div>
            {/* 节点卡 */}
            <button
              onClick={() => {
                setExpandedIdx((prev) => {
                  const next = new Set(prev)
                  if (next.has(idx)) next.delete(idx)
                  else next.add(idx)
                  return next
                })
              }}
              className={`flex-1 min-w-0 mb-1.5 rounded-lg border bg-[var(--bg-primary)] px-2.5 py-1.5 text-left hover:bg-[var(--bg-hover)] transition-colors ${tone.border}`}
            >
              <div className="flex items-center gap-2">
                <span className="font-mono text-[11.5px] font-medium truncate">{n.name}</span>
                {isRetry && (
                  <span className="shrink-0 text-[9.5px] px-1 py-px rounded bg-[var(--warning)]/15 text-[var(--warning)]">
                    ↻ {t('home.timelineRetry')}
                  </span>
                )}
                <span className={`ml-auto shrink-0 text-[10px] tabular-nums ${tone.label}`}>
                  {n.ok === true ? '✓' : n.ok === false ? '✗' : '…'}
                  {n.durationMs != null && ` ${(n.durationMs / 1000).toFixed(1)}s`}
                </span>
              </div>
              {n.args && <div className="mt-0.5 font-mono text-[10px] text-[var(--text-muted)] truncate">{n.args.length > 100 ? `${n.args.slice(0, 100)}…` : n.args}</div>}
              {expanded && n.output && (
                <pre className="mt-1.5 px-2 py-1.5 rounded bg-[var(--bg-window)] text-[10px] leading-4 text-[var(--text-secondary)] whitespace-pre-wrap break-all max-h-40 overflow-y-auto">
                  {n.output}
                </pre>
              )}
            </button>
          </div>
        )
      })}
    </div>
  )
}


