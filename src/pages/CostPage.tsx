import { useState, useEffect, useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { getCostSummary, getDailyUsage, getTaskStats, getTaskRuns, type CostSummary, type DailyUsage, type TaskStats, type TaskRun } from '../api/cost'
import { queryBalances, type ProviderBalance } from '../api/balance'
import { sendNotification } from '../api/desktop'
import Icon from '../icons/Icon'

export default function CostPage() {
  const { t } = useTranslation()
  const [summary, setSummary] = useState<CostSummary | null>(null)
  const [daily, setDaily] = useState<DailyUsage[]>([])
  const [taskStats, setTaskStats] = useState<TaskStats | null>(null)
  const [taskRuns, setTaskRuns] = useState<TaskRun[]>([])
  const [runFilter, setRunFilter] = useState('')
  // 服务商余额/额度（实时查询各 provider 的 billing 接口）
  const [balances, setBalances] = useState<ProviderBalance[]>([])
  const [balanceLoading, setBalanceLoading] = useState(false)
  // 余额查询是否走系统代理（默认直连）
  const [balanceUseProxy, setBalanceUseProxy] = useState(false)

  const loadBalances = useCallback(async () => {
    setBalanceLoading(true)
    try {
      const list = await queryBalances(undefined, balanceUseProxy)
      setBalances(list)
      // 低余额告警：剩余 < 总额 10% 或剩余 < 1（按币种）视为低余额；每服务商 24h 提醒一次
      const now = Date.now()
      const alerted: Record<string, number> = JSON.parse(
        localStorage.getItem('deveco-balance-alerted') ?? '{}',
      )
      for (const b of list) {
        if (!b.ok || b.remaining == null) continue
        const total = b.total ?? 0
        const ratio = total > 0 ? b.remaining / total : b.remaining > 0 ? 1 : 0
        const lowAbsolute = (b.currency === 'CNY' ? b.remaining < 1 : b.remaining < 0.2) && b.remaining > 0
        const isLow = b.exhausted || ratio < 0.1 || lowAbsolute
        if (!isLow) continue
        const last = alerted[b.provider_id] ?? 0
        if (now - last < 24 * 3600 * 1000) continue
        alerted[b.provider_id] = now
        const cur = b.currency ?? 'USD'
        const sym = cur === 'CNY' ? '¥' : cur === 'USD' ? '$' : ''
        void sendNotification(
          b.exhausted ? t('cost.balanceAlertExhaustedTitle') : t('cost.balanceAlertLowTitle'),
          t('cost.balanceAlertBody', {
            name: b.provider_name,
            amount: `${sym}${b.remaining.toFixed(2)} ${cur}`,
          }),
          b.exhausted ? 'error' : 'info',
        )
      }
      localStorage.setItem('deveco-balance-alerted', JSON.stringify(alerted))
    } catch (e) {
      console.error(e)
      setBalances([])
    } finally {
      setBalanceLoading(false)
    }
  }, [t, balanceUseProxy])

  const load = async () => {
    const today = new Date()
    const start = new Date(today)
    start.setDate(start.getDate() - 30)

    const range = {
      start: start.toISOString().split('T')[0],
      end: today.toISOString().split('T')[0],
    }

    try {
      const s = await getCostSummary(range)
      setSummary(s)
      const d = await getDailyUsage(range)
      setDaily(d)
    } catch (e) {
      console.error(e)
    }
    // 任务级指标（成功率 / 耗时分布 / 错误分类），失败不影响主面板
    try {
      setTaskStats(await getTaskStats(undefined, 30))
    } catch (e) {
      console.error(e)
    }
    // 最近任务明细（trace 列表）
    try {
      setTaskRuns(await getTaskRuns(undefined, '', 20))
    } catch (e) {
      console.error(e)
    }
  }

  useEffect(() => { load() }, [])
  useEffect(() => { loadBalances() }, [loadBalances])

  /** 错误分类 → 可读标签（与后端 errors::ErrorKind::as_str 对应） */
  const errKindLabel = (kind: string) => {
    const map: Record<string, string> = {
      auth: t('cost.errKind.auth'),
      rate_limited: t('cost.errKind.rate_limited'),
      context_overflow: t('cost.errKind.context_overflow'),
      server: t('cost.errKind.server'),
      network: t('cost.errKind.network'),
      timeout: t('cost.errKind.timeout'),
      client: t('cost.errKind.client'),
      local: t('cost.errKind.local'),
      unknown: t('cost.errKind.unknown'),
    }
    return map[kind] ?? kind
  }

  return (
    <div>
      <h2 className="text-xl font-semibold mb-6">{t('cost.title')}</h2>

      {/* 服务商余额/额度：实时查询各 provider 的 billing 接口 */}
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-[var(--text-secondary)]">{t('cost.balances')}</h3>
        <div className="flex items-center gap-3">
          <label className="flex items-center gap-1.5 text-[11px] text-[var(--text-secondary)] cursor-pointer select-none">
            <input
              type="checkbox"
              checked={balanceUseProxy}
              onChange={(e) => setBalanceUseProxy(e.target.checked)}
              className="accent-[var(--accent)]"
            />
            {t('cost.useProxy')}
          </label>
          <button
            onClick={loadBalances}
            disabled={balanceLoading}
            className="flex items-center gap-1 text-[11px] text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-50"
          >
            <Icon name="refresh" size={12} className={balanceLoading ? 'animate-spin' : ''} />
            {t('cost.refreshBalance')}
          </button>
        </div>
      </div>
      <div className="grid grid-cols-3 gap-3 mb-6">
        {balanceLoading && balances.length === 0 ? (
          [0, 1, 2].map((i) => (
            <div key={i} className="h-24 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] animate-pulse" />
          ))
        ) : balances.length === 0 ? (
          <div className="col-span-3 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] px-4 py-6 text-center text-sm text-[var(--text-secondary)]">
            {t('cost.noProviders')}
          </div>
        ) : (
          balances.map((b) => (
            <BalanceCard key={b.provider_id} b={b} />
          ))
        )}
      </div>

      <div className="grid grid-cols-4 gap-4 mb-6">
        <StatCard label={t('cost.totalRequests')} value={summary?.total_requests ?? 0} />
        <StatCard label={t('cost.inputTokens')} value={formatTokens(summary?.total_input_tokens ?? 0)} />
        <StatCard label={t('cost.outputTokens')} value={formatTokens(summary?.total_output_tokens ?? 0)} />
        <StatCard label={t('cost.totalCost')} value={`¥${(summary?.total_cost_cny ?? 0).toFixed(2)}`} />
      </div>

      {/* 任务级指标：Agent 任务成功率 / 耗时分布 / 错误分类 */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('cost.taskStats')}</h3>
      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4 mb-6">
        {!taskStats || taskStats.total_tasks === 0 ? (
          <p className="text-sm text-[var(--text-secondary)]">{t('cost.taskEmpty')}</p>
        ) : (
          <div className="space-y-4">
            <div className="grid grid-cols-4 gap-4">
              <StatCard label={t('cost.taskTotal')} value={taskStats.total_tasks} />
              <StatCard
                label={t('cost.taskSuccessRate')}
                value={`${(taskStats.success_rate * 100).toFixed(1)}%`}
                tone={taskStats.success_rate >= 0.9 ? 'ok' : taskStats.success_rate >= 0.7 ? 'warn' : 'bad'}
              />
              <StatCard
                label={t('cost.taskP50')}
                value={taskStats.p50_ms != null ? formatDuration(taskStats.p50_ms, t('cost.minutes')) : '—'}
              />
              <StatCard
                label={t('cost.taskP95')}
                value={taskStats.p95_ms != null ? formatDuration(taskStats.p95_ms, t('cost.minutes')) : '—'}
              />
            </div>
            <div className="grid grid-cols-4 gap-4 text-sm">
              <div>
                <p className="text-xs text-[var(--text-secondary)] mb-1">{t('cost.taskLegend')}</p>
                <p className="text-[var(--success)]">{taskStats.success_count}</p>
                <p className="text-[var(--danger)]">{taskStats.error_count}</p>
                <p className="text-[var(--text-secondary)]">{taskStats.cancelled_count}</p>
              </div>
              <div>
                <p className="text-xs text-[var(--text-secondary)] mb-1">{t('cost.taskAvg')}</p>
                <p>{taskStats.avg_duration_ms != null ? formatDuration(taskStats.avg_duration_ms, t('cost.minutes')) : '—'}</p>
              </div>
              <div>
                <p className="text-xs text-[var(--text-secondary)] mb-1">{t('cost.taskCost')}</p>
                <p>¥{taskStats.total_cost_cny.toFixed(2)}</p>
              </div>
              <div>
                <p className="text-xs text-[var(--text-secondary)] mb-1">{t('cost.taskTokens')}</p>
                <p>
                  {formatTokens(taskStats.total_input_tokens)} / {formatTokens(taskStats.total_output_tokens)}
                </p>
              </div>
            </div>
            {taskStats.by_error_kind.length > 0 && (
              <div>
                <p className="text-xs text-[var(--text-secondary)] mb-1.5">{t('cost.taskErrorKind')}</p>
                <div className="flex flex-wrap gap-2">
                  {taskStats.by_error_kind.map((e) => (
                    <span
                      key={e.kind}
                      className="px-2 py-0.5 rounded-md bg-[var(--danger)]/10 text-[var(--danger)] text-[11px]"
                    >
                      {errKindLabel(e.kind)} × {e.count}
                    </span>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* 按模型统计：费用追踪按模型分组（请求数 / Token / 费用） */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('cost.byModel')}</h3>
      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg overflow-hidden mb-6">
        {!summary || summary.by_model.length === 0 ? (
          <p className="px-4 py-8 text-center text-sm text-[var(--text-secondary)]">{t('cost.noData')}</p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-[var(--border)] text-[var(--text-secondary)]">
                <th className="text-left px-4 py-2">{t('cost.model')}</th>
                <th className="text-right px-4 py-2">{t('cost.requests')}</th>
                <th className="text-right px-4 py-2">{t('cost.input')}</th>
                <th className="text-right px-4 py-2">{t('cost.output')}</th>
                <th className="text-right px-4 py-2">{t('cost.fee')}</th>
                <th className="text-left px-4 py-2 w-1/3">{t('cost.share')}</th>
              </tr>
            </thead>
            <tbody>
              {summary.by_model.map((m) => {
                const ratio =
                  summary.total_cost_cny > 0
                    ? (m.total_cost_cny / summary.total_cost_cny) * 100
                    : 0
                return (
                  <tr key={m.model} className="border-b border-[var(--border)] last:border-0">
                    <td className="px-4 py-2 max-w-[240px] truncate" title={m.model}>
                      {m.model}
                    </td>
                    <td className="px-4 py-2 text-right">{m.request_count}</td>
                    <td className="px-4 py-2 text-right">{formatTokens(m.input_tokens)}</td>
                    <td className="px-4 py-2 text-right">{formatTokens(m.output_tokens)}</td>
                    <td className="px-4 py-2 text-right whitespace-nowrap">¥{m.total_cost_cny.toFixed(4)}</td>
                    <td className="px-4 py-2">
                      <div className="flex items-center gap-2">
                        <div className="flex-1 h-1.5 rounded-full bg-[var(--bg-hover)] overflow-hidden">
                          <div
                            className="h-full rounded-full bg-[var(--accent)]"
                            style={{ width: `${Math.min(ratio, 100)}%` }}
                          />
                        </div>
                        <span className="text-xs text-[var(--text-secondary)] w-10 text-right">
                          {ratio.toFixed(1)}%
                        </span>
                      </div>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        )}
      </div>

      {/* 最近任务明细（trace 列表：每次 Agent 任务一行） */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('cost.recentTasks')}</h3>
      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg overflow-hidden mb-6">
        <div className="flex items-center gap-2 px-4 py-2 border-b border-[var(--border)]">
          <span className="text-xs text-[var(--text-secondary)]">{t('cost.filter')}</span>
          {['', 'success', 'error', 'cancelled'].map((s) => (
            <button
              key={s}
              onClick={() => {
                setRunFilter(s)
                getTaskRuns(undefined, s, 20)
                  .then(setTaskRuns)
                  .catch(() => {})
              }}
              className={`px-2 py-0.5 rounded-md text-[11px] transition-colors ${
                runFilter === s
                  ? 'bg-[var(--accent)] text-white'
                  : 'text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
              }`}
            >
              {s === '' ? t('cost.all') : s === 'success' ? t('cost.success') : s === 'error' ? t('cost.failed') : t('cost.cancelled')}
            </button>
          ))}
        </div>
        {taskRuns.length === 0 ? (
          <p className="px-4 py-8 text-center text-sm text-[var(--text-secondary)]">
            {t('cost.runsEmpty')}
          </p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-[var(--border)] text-[var(--text-secondary)]">
                <th className="text-left px-4 py-2">{t('cost.time')}</th>
                <th className="text-left px-4 py-2">{t('cost.status')}</th>
                <th className="text-left px-4 py-2">{t('cost.model')}</th>
                <th className="text-right px-4 py-2">{t('cost.duration')}</th>
                <th className="text-right px-4 py-2">{t('cost.retry')}</th>
                <th className="text-right px-4 py-2">{t('cost.toolRounds')}</th>
                <th className="text-right px-4 py-2">{t('cost.cost')}</th>
                <th className="text-left px-4 py-2">{t('cost.error')}</th>
              </tr>
            </thead>
            <tbody>
              {taskRuns.map((r) => (
                <tr key={r.id} className="border-b border-[var(--border)] last:border-0 align-top">
                  <td className="px-4 py-2 whitespace-nowrap">{formatDateTime(r.started_at)}</td>
                  <td className="px-4 py-2">
                    <span
                      className="px-1.5 py-0.5 rounded text-[11px]"
                      style={{
                        backgroundColor:
                          r.status === 'success'
                            ? 'var(--success)'
                            : r.status === 'cancelled'
                              ? 'var(--text-secondary)'
                              : 'var(--danger)',
                        color: '#fff',
                        opacity: r.status === 'cancelled' ? 0.6 : 1,
                      }}
                    >
                      {r.status === 'success' ? t('cost.success') : r.status === 'cancelled' ? t('cost.cancelled') : t('cost.failed')}
                    </span>
                  </td>
                  <td className="px-4 py-2 max-w-[180px] truncate" title={r.model ?? ''}>
                    {r.model ?? '—'}
                  </td>
                  <td className="px-4 py-2 text-right whitespace-nowrap">
                    {formatDuration(r.duration_ms, t('cost.minutes'))}
                  </td>
                  <td className="px-4 py-2 text-right">{r.retry_count > 0 ? r.retry_count : '—'}</td>
                  <td className="px-4 py-2 text-right">{r.tool_rounds > 0 ? r.tool_rounds : '—'}</td>
                  <td className="px-4 py-2 text-right whitespace-nowrap">
                    ¥{r.cost_cny.toFixed(4)}
                  </td>
                  <td className="px-4 py-2 max-w-[220px]">
                    {r.error_kind ? (
                      <span
                        className="px-1.5 py-0.5 rounded text-[11px] bg-[var(--danger)]/10 text-[var(--danger)]"
                        title={r.error_message ?? ''}
                      >
                        {errKindLabel(r.error_kind)}
                      </span>
                    ) : (
                      '—'
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('cost.daily')}</h3>
      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-[var(--border)] text-[var(--text-secondary)]">
              <th className="text-left px-4 py-2">{t('cost.date')}</th>
              <th className="text-right px-4 py-2">{t('cost.requests')}</th>
              <th className="text-right px-4 py-2">{t('cost.input')}</th>
              <th className="text-right px-4 py-2">{t('cost.output')}</th>
              <th className="text-right px-4 py-2">{t('cost.fee')}</th>
            </tr>
          </thead>
          <tbody>
            {daily.length === 0 ? (
              <tr><td colSpan={5} className="px-4 py-8 text-center text-[var(--text-secondary)]">{t('cost.noData')}</td></tr>
            ) : (
              daily.map((d, i) => (
                <tr key={i} className="border-b border-[var(--border)] last:border-0">
                  <td className="px-4 py-2">{d.date}</td>
                  <td className="px-4 py-2 text-right">{d.request_count}</td>
                  <td className="px-4 py-2 text-right">{formatTokens(d.input_tokens)}</td>
                  <td className="px-4 py-2 text-right">{formatTokens(d.output_tokens)}</td>
                  <td className="px-4 py-2 text-right">¥{d.total_cost_cny.toFixed(4)}</td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  )
}

/** 服务商余额卡片：展示剩余额度、总额/已用进度条；查询失败显示原因 */
function BalanceCard({ b }: { b: ProviderBalance }) {
  const { t } = useTranslation()
  const cur = b.currency ?? 'USD'
  const sym = cur === 'CNY' ? '¥' : cur === 'USD' ? '$' : ''
  const pct =
    b.total && b.total > 0 && b.remaining != null
      ? Math.max(0, Math.min(100, (b.remaining / b.total) * 100))
      : null
  const tone = !b.ok
    ? 'var(--text-muted)'
    : b.exhausted
      ? 'var(--danger)'
      : pct != null && pct < 20
        ? 'var(--warning)'
        : 'var(--success)'
  return (
    <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4">
      <div className="flex items-center justify-between mb-2">
        <p className="text-xs text-[var(--text-secondary)] truncate" title={b.provider_name}>{b.provider_name}</p>
        {b.ok ? (
          <span
            className="w-2 h-2 rounded-full shrink-0"
            style={{ background: tone }}
            title={b.exhausted ? t('cost.balanceExhausted') : t('cost.balanceAvailable')}
          />
        ) : (
          <Icon name="info" size={12} className="text-[var(--text-muted)] shrink-0" />
        )}
      </div>
      {b.ok ? (
        <>
          <p className="text-lg font-semibold" style={{ color: tone }}>
            {sym}{b.remaining != null ? b.remaining.toFixed(2) : '—'}
            <span className="text-xs text-[var(--text-muted)] font-normal ml-1">{cur}</span>
          </p>
          {pct != null && (
            <div className="mt-2 h-1.5 rounded-full bg-[var(--bg-hover)] overflow-hidden">
              <div className="h-full rounded-full transition-all" style={{ width: `${pct}%`, background: tone }} />
            </div>
          )}
          <div className="mt-1.5 flex items-center justify-between text-[10px] text-[var(--text-muted)]">
            <span>{t('cost.balanceUsed')}: {sym}{b.used != null ? b.used.toFixed(2) : '—'}</span>
            <span>{t('cost.balanceTotal')}: {sym}{b.total != null ? b.total.toFixed(2) : '—'}</span>
          </div>
        </>
      ) : (
        <p className="text-[11px] text-[var(--text-muted)] leading-relaxed mt-2 line-clamp-3" title={b.error ?? ''}>
          {b.error ?? t('cost.balanceUnsupported')}
        </p>
      )}
    </div>
  )
}

function StatCard({ label, value, tone }: { label: string; value: string | number; tone?: 'ok' | 'warn' | 'bad' }) {
  const color = tone === 'ok' ? 'var(--success)' : tone === 'warn' ? 'var(--warning)' : tone === 'bad' ? 'var(--danger)' : undefined
  return (
    <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4">
      <p className="text-xs text-[var(--text-secondary)]">{label}</p>
      <p className="text-lg font-semibold mt-1" style={color ? { color } : undefined}>{value}</p>
    </div>
  )
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return String(n)
}

function formatDuration(ms: number, minUnit: string): string {
  if (ms >= 60_000) return `${(ms / 60_000).toFixed(1)}${minUnit}`
  if (ms >= 1_000) return `${(ms / 1_000).toFixed(1)}s`
  return `${ms}ms`
}

/** 时间戳（秒）→ MM/DD HH:mm */
function formatDateTime(ts: number): string {
  const d = new Date(ts * 1000)
  const mm = String(d.getMonth() + 1).padStart(2, '0')
  const dd = String(d.getDate()).padStart(2, '0')
  const hh = String(d.getHours()).padStart(2, '0')
  const mi = String(d.getMinutes()).padStart(2, '0')
  return `${mm}/${dd} ${hh}:${mi}`
}
