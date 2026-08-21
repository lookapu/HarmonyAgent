import { useState, useEffect, useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { getCostSummary, getDailyUsage, getTaskStats, getTaskRuns, getAllBudgetStatus, getRequestLogs, type CostSummary, type DailyUsage, type TaskStats, type TaskRun, type AllBudgetStatus, type RequestLog } from '../api/cost'
import { queryBalances, type ProviderBalance } from '../api/balance'
import { sendNotification } from '../api/desktop'
import { useNotificationStore } from '../stores/notificationStore'
import { save as dialogSave } from '@tauri-apps/plugin-dialog'
import { writeTextFile } from '@tauri-apps/plugin-fs'
import Icon from '../icons/Icon'
import { getJSON, setJSON } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'
import { getAgentSloPolicy, getReliabilityDashboard, listAgentAlerts, runReliabilityEvaluation, type AgentAlert, type ReliabilityDashboard, type SloPolicy } from '../api/reliability'

/** CSV 字段转义：含逗号/引号/换行的字段用双引号包裹，内部双引号 → 双重转义 */
const csvEscape = (s: string) => {
  if (/[",\n]/.test(s)) return `"${s.replace(/"/g, '""')}"`
  return s
}

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
  // 是否存在"已配置但未提供余额接口"的服务商（决定空态文案：无服务商 vs 全不支持）
  const [hasBalanceUnsupported, setHasBalanceUnsupported] = useState(false)
  // 余额查询是否走系统代理（默认直连）
  const [balanceUseProxy, setBalanceUseProxy] = useState(false)
  // 预算状态（跨 Provider 汇总）
  const [budget, setBudget] = useState<AllBudgetStatus | null>(null)
  // LLM 请求级 trace：每次 LLM 调用一行（比 task_runs 更细：能看到 latency_ms / first_token_ms / cache 命中）
  const [requestLogs, setRequestLogs] = useState<RequestLog[]>([])
  const [requestLogsLoading, setRequestLogsLoading] = useState(false)
  const [reliability, setReliability] = useState<ReliabilityDashboard | null>(null)
  const [reliabilityLoading, setReliabilityLoading] = useState(false)
  const [agentAlerts, setAgentAlerts] = useState<AgentAlert[]>([])
  const [sloPolicy, setSloPolicy] = useState<SloPolicy | null>(null)
  // 请求日志状态过滤：all / success / error
  const [logStatusFilter, setLogStatusFilter] = useState<'all' | 'success' | 'error'>('all')
  // 请求日志分页
  const [logPage, setLogPage] = useState(0)
  const LOG_PAGE_SIZE = 50

  const loadBalances = useCallback(async () => {
    setBalanceLoading(true)
    try {
      const list = await queryBalances(undefined, balanceUseProxy)
      // 未提供余额查询接口的服务商（unsupported）直接过滤，不展示余额卡片
      const supported = list.filter((b) => !b.unsupported)
      setHasBalanceUnsupported(list.length > supported.length)
      setBalances(supported)
      // 低余额告警：剩余 < 总额 10% 或剩余 < 1（按币种）视为低余额；每服务商 24h 提醒一次
      const now = Date.now()
      const alerted: Record<string, number> = getJSON<Record<string, number>>(STORAGE_KEYS.BALANCE_ALERTED, {})
      for (const b of supported) {
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
      setJSON(STORAGE_KEYS.BALANCE_ALERTED, alerted)
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

  const loadBudget = useCallback(async () => {
    try {
      setBudget(await getAllBudgetStatus())
    } catch (e) {
      console.error(e)
    }
  }, [])

  const loadReliability = useCallback(async () => {
    try {
      const dashboard = await getReliabilityDashboard(30)
      const alerts = await listAgentAlerts(20)
      const policy = await getAgentSloPolicy()
      setReliability(dashboard)
      setAgentAlerts(alerts)
      setSloPolicy(policy)
    } catch (e) {
      console.error(e)
    }
  }, [])

  const runReliabilityGate = useCallback(async () => {
    setReliabilityLoading(true)
    try {
      const result = await runReliabilityEvaluation(0.95)
      await loadReliability()
      useNotificationStore.getState().push({
        tone: result.passed ? 'success' : 'error',
        title: result.passed ? t('cost.reliabilityGatePassed') : t('cost.reliabilityGateFailed'),
        body: `${result.passed_cases}/${result.total_cases} · ${(result.score * 100).toFixed(1)}%`,
      })
    } catch (e) {
      console.error(e)
    } finally {
      setReliabilityLoading(false)
    }
  }, [loadReliability, t])

  /** 加载请求级 trace（按 status 过滤、按分页偏移拉）
   *  - 一次性拉到 200 条（满足前端翻 4 页），按 client 端 status 过滤，避免每页都打后端
   *  - status_code 2xx → success；4xx/5xx 或有 error_message → error
   */
  const loadRequestLogs = useCallback(async (page = 0) => {
    setRequestLogsLoading(true)
    try {
      const offset = page * LOG_PAGE_SIZE
      const list = await getRequestLogs({ limit: LOG_PAGE_SIZE, offset })
      setRequestLogs(list)
    } catch (e) {
      console.error(e)
      setRequestLogs([])
    } finally {
      setRequestLogsLoading(false)
    }
  }, [])

  useEffect(() => { load() }, [])
  useEffect(() => { loadBalances() }, [loadBalances])
  useEffect(() => { loadBudget() }, [loadBudget])
  useEffect(() => { loadReliability() }, [loadReliability])
  useEffect(() => { loadRequestLogs(logPage) }, [loadRequestLogs, logPage])

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

  /** 导出当前显示范围的账单为 CSV（纯前端：组装当前 summary/daily 字段 → 写文件）
   *  - 包含两个段：按模型汇总 + 按日汇总，用空行分隔
   *  - CSV 用逗号分隔 + 双引号包裹文本字段（BOM 头让 Excel 中文不乱码）
   */
  const exportBillingCsv = async () => {
    try {
      const lines: string[] = []
      lines.push('\uFEFF') // UTF-8 BOM
      // 段 1：按模型汇总
      lines.push('Section,Model,Requests,Input Tokens,Output Tokens,Total Cost (CNY)')
      if (summary) {
        for (const m of summary.by_model) {
          lines.push([
            'by_model',
            csvEscape(m.model),
            String(m.request_count),
            String(m.input_tokens),
            String(m.output_tokens),
            m.total_cost_cny.toFixed(6),
          ].join(','))
        }
      }
      lines.push('')
      // 段 2：按 Provider 汇总
      lines.push('Section,Provider,Requests,Total Cost (CNY)')
      if (summary) {
        for (const p of summary.by_provider) {
          lines.push([
            'by_provider',
            csvEscape(p.provider_name),
            String(p.request_count),
            p.total_cost_cny.toFixed(6),
          ].join(','))
        }
      }
      lines.push('')
      // 段 3：按日汇总
      lines.push('Section,Date,Provider,Model,Requests,Input Tokens,Output Tokens,Total Cost (CNY)')
      for (const d of daily) {
        lines.push([
          'daily',
          d.date,
          csvEscape(d.provider_id ?? ''),
          csvEscape(d.model ?? ''),
          String(d.request_count),
          String(d.input_tokens),
          String(d.output_tokens),
          d.total_cost_cny.toFixed(6),
        ].join(','))
      }
      const csv = lines.join('\n')
      const ts = new Date()
      const fname = `billing-${ts.getFullYear()}${String(ts.getMonth() + 1).padStart(2, '0')}${String(ts.getDate()).padStart(2, '0')}.csv`
      const path = await dialogSave({
        defaultPath: fname,
        filters: [{ name: 'CSV', extensions: ['csv'] }],
      })
      if (!path) return
      await writeTextFile(path, csv)
      useNotificationStore.getState().push({
        tone: 'success',
        title: t('cost.exportSuccess'),
        body: fname,
      })
    } catch (e) {
      console.error(e)
    }
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-semibold">{t('cost.title')}</h2>
        <button
          onClick={exportBillingCsv}
          disabled={!summary || summary.total_requests === 0}
          className="flex items-center gap-1.5 h-8 px-3 rounded-lg border border-[var(--border)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed text-[12px] text-[var(--text-secondary)] transition-colors"
          title={t('cost.exportBilling')}
        >
          <Icon name="download" size={13} />
          {t('cost.exportBilling')}
        </button>
      </div>

      {/* 预算总览：日/月已用 vs 限额——后端预算门控已达上限时这里会先看到红色告警 */}
      {budget && (
        <div className="grid grid-cols-2 gap-3 mb-6">
          <BudgetMeter
            label={t('cost.budgetDaily')}
            used={budget.used_today_cny}
            limit={budget.daily_limit_cny}
          />
          <BudgetMeter
            label={t('cost.budgetMonthly')}
            used={budget.used_month_cny}
            limit={budget.monthly_limit_cny}
          />
        </div>
      )}

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
            {hasBalanceUnsupported ? t('cost.noBalances') : t('cost.noProviders')}
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

      {/* Agent 可靠性控制面：验收证据、恢复、调度、DAG 与版本化评测统一观测 */}
      <div className="flex items-center justify-between mb-3">
        <div>
          <h3 className="text-sm font-medium text-[var(--text-secondary)]">{t('cost.reliabilityTitle')}</h3>
          <p className="text-[11px] text-[var(--text-muted)] mt-0.5">{t('cost.reliabilityDesc')}</p>
        </div>
        <button
          onClick={runReliabilityGate}
          disabled={reliabilityLoading}
          className="flex items-center gap-1.5 h-8 px-3 rounded-lg border border-[var(--border)] hover:bg-[var(--bg-hover)] disabled:opacity-50 text-[12px] transition-colors"
        >
          <Icon name="refresh" size={12} className={reliabilityLoading ? 'animate-spin' : ''} />
          {t('cost.runReliabilityGate')}
        </button>
      </div>
      <div className="modern-card rounded-lg p-4 mb-6 space-y-4">
        <div className="grid grid-cols-4 gap-4">
          <StatCard label={t('cost.acceptanceRate')} value={reliability?.total_runs ? `${(reliability.acceptance_rate * 100).toFixed(1)}%` : '—'} tone={reliability?.total_runs ? rateTone(reliability.acceptance_rate) : undefined} />
          <StatCard label={t('cost.qualityScore')} value={reliability?.total_runs ? reliability.average_quality_score.toFixed(1) : '—'} tone={reliability?.total_runs ? scoreTone(reliability.average_quality_score) : undefined} />
          <StatCard label={t('cost.evidenceCoverage')} value={`${((reliability?.structured_evidence_coverage ?? 0) * 100).toFixed(1)}%`} tone={rateTone(reliability?.structured_evidence_coverage)} />
          <StatCard label={t('cost.falseCompletions')} value={reliability?.false_completion_count ?? 0} tone={(reliability?.false_completion_count ?? 0) === 0 ? 'ok' : 'bad'} />
        </div>
        <div className="grid grid-cols-4 gap-4 text-sm">
          <ReliabilityValue label={t('cost.remediationSuccess')} value={`${((reliability?.remediation_success_rate ?? 0) * 100).toFixed(1)}%`} />
          <ReliabilityValue label={t('cost.recoverySuccess')} value={`${((reliability?.recovery_success_rate ?? 0) * 100).toFixed(1)}%`} />
          <ReliabilityValue label={t('cost.dagProgress')} value={`${reliability?.dag_completed_nodes ?? 0}/${reliability?.dag_total_nodes ?? 0}`} />
          <ReliabilityValue label={t('cost.duplicateEffects')} value={String(reliability?.duplicate_side_effect_count ?? 0)} />
        </div>
        <div className="grid grid-cols-4 gap-4 text-sm border-t border-[var(--border)] pt-3">
          <ReliabilityValue label={t('cost.openAlerts')} value={String(reliability?.open_alert_count ?? 0)} />
          <ReliabilityValue label={t('cost.criticalAlerts')} value={String(reliability?.critical_alert_count ?? 0)} />
          <ReliabilityValue label={t('cost.monthlyRuns')} value={String(reliability?.quota.runs ?? 0)} />
          <ReliabilityValue label={t('cost.monthlyToolCalls')} value={String(reliability?.quota.tool_calls ?? 0)} />
        </div>
        <div className="grid grid-cols-4 gap-4 text-sm border-t border-[var(--border)] pt-3">
          <ReliabilityValue label={t('cost.activeWorkers')} value={String(reliability?.worker_runtime.active_workers ?? 0)} />
          <ReliabilityValue label={t('cost.runningWorkerTasks')} value={String(reliability?.worker_runtime.running_tasks ?? 0)} />
          <ReliabilityValue label={t('cost.recoveredWorkerTasks')} value={String(reliability?.worker_runtime.recovered_tasks ?? 0)} />
          <ReliabilityValue label={t('cost.lostWorkers')} value={String(reliability?.worker_runtime.lost_workers ?? 0)} />
        </div>
        <div className="flex flex-wrap items-center gap-2 border-t border-[var(--border)] pt-3 text-[11px]">
          <span className="text-[var(--text-muted)]">{t('cost.schedulerStates')}</span>
          {(reliability?.scheduler_states ?? []).map((item) => (
            <span key={item.name} className="badge-tone badge-tone-info">{item.name} · {item.count}</span>
          ))}
          {(reliability?.scheduler_states.length ?? 0) === 0 && <span className="text-[var(--text-muted)]">—</span>}
          <span className="ml-auto text-[var(--text-muted)]">
            {t('cost.latestEval')}: {reliability?.latest_eval
              ? `${reliability.latest_eval.platform} · ${(reliability.latest_eval.score * 100).toFixed(1)}%`
              : t('cost.notRun')}
          </span>
        </div>
        {sloPolicy?.enabled && (
          <p className="text-[10.5px] text-[var(--text-muted)]">
            {t('cost.sloTargets', { acceptance: (sloPolicy.acceptance_target * 100).toFixed(0), recovery: (sloPolicy.recovery_target * 100).toFixed(0), evidence: (sloPolicy.evidence_target * 100).toFixed(0) })}
          </p>
        )}
        {agentAlerts.length > 0 && (
          <div className="border-t border-[var(--border)] pt-3">
            <p className="text-xs text-[var(--text-secondary)] mb-2">{t('cost.recentAlerts')}</p>
            <div className="space-y-1.5 max-h-36 overflow-auto">
              {agentAlerts.slice(0, 8).map((alert) => (
                <div key={alert.alert_id} className="flex items-center gap-2 text-[11px]">
                  <span className={`badge-tone ${alert.severity === 'critical' ? 'badge-tone-bad' : 'badge-tone-warn'}`}>{alert.severity}</span>
                  <span className="font-mono text-[var(--text-secondary)]">{alert.code}</span>
                  <span className="truncate" title={alert.message}>{alert.message}</span>
                  <span className="ml-auto shrink-0 text-[var(--text-muted)]">{new Date(alert.created_at).toLocaleString()}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* 任务级指标：Agent 任务成功率 / 耗时分布 / 错误分类 */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('cost.taskStats')}</h3>
      <div className="modern-card rounded-lg p-4 mb-6">
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
      <div className="modern-card rounded-lg overflow-hidden mb-6">
        {!summary || summary.by_model.length === 0 ? (
          <p className="px-4 py-8 text-center text-sm text-[var(--text-secondary)]">{t('cost.noData')}</p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-[var(--border)] text-[var(--text-secondary)]">
                <th className="text-left px-4 py-2">{t('cost.model')}</th>
                <th className="text-right px-4 py-2 tnum">{t('cost.requests')}</th>
                <th className="text-right px-4 py-2 tnum">{t('cost.input')}</th>
                <th className="text-right px-4 py-2 tnum">{t('cost.output')}</th>
                <th className="text-right px-4 py-2 tnum">{t('cost.fee')}</th>
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
      <div className="modern-card rounded-lg overflow-hidden mb-6">
        <div className="flex items-center gap-2 px-4 py-2 border-b border-[var(--border)]">
          <span className="text-xs text-[var(--text-secondary)]">{t('cost.filter')}</span>
          {['', 'success', 'incomplete', 'error', 'cancelled'].map((s) => (
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
                  ? 'tab-active'
                  : 'tab-inactive'
              }`}
            >
              {s === ''
                ? t('cost.all')
                : s === 'success'
                  ? t('cost.success')
                  : s === 'incomplete'
                    ? t('cost.incomplete')
                    : s === 'error'
                      ? t('cost.failed')
                      : t('cost.cancelled')}
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
                <th className="text-right px-4 py-2 tnum">{t('cost.duration')}</th>
                <th className="text-right px-4 py-2 tnum">{t('cost.retry')}</th>
                <th className="text-right px-4 py-2 tnum">{t('cost.toolRounds')}</th>
                <th className="text-right px-4 py-2 tnum">{t('cost.cost')}</th>
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
                            : r.status === 'incomplete'
                              ? 'var(--warning)'
                            : r.status === 'cancelled'
                              ? 'var(--text-secondary)'
                              : 'var(--danger)',
                        color: '#fff',
                        opacity: r.status === 'cancelled' ? 0.6 : 1,
                      }}
                    >
                      {r.status === 'success'
                        ? t('cost.success')
                        : r.status === 'incomplete'
                          ? t('cost.incomplete')
                          : r.status === 'cancelled'
                            ? t('cost.cancelled')
                            : t('cost.failed')}
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

      {/* LLM 请求级 trace：单次 API 调用 = 一行；status_code 2xx = 成功；4xx/5xx 或有 error_message = 失败
       *  - 比 task_runs 更细：latency / first_token_ms / cache_read+creation 都能看到
       *  - 状态过滤 + 分页翻页（每页 50 条）
       */}
      <div className="flex items-center justify-between mb-3 mt-2">
        <h3 className="text-sm font-medium text-[var(--text-secondary)]">{t('cost.requestLogs')}</h3>
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-1.5 text-[11px]">
            {(['all', 'success', 'error'] as const).map((s) => (
              <button
                key={s}
                onClick={() => setLogStatusFilter(s)}
                className={`px-2 py-0.5 rounded-md transition-colors ${
                  logStatusFilter === s ? 'tab-active' : 'tab-inactive'
                }`}
              >
                {s === 'all' ? t('cost.all') : s === 'success' ? t('cost.success') : t('cost.failed')}
              </button>
            ))}
          </div>
          <button
            onClick={() => loadRequestLogs(logPage)}
            disabled={requestLogsLoading}
            className="flex items-center gap-1 text-[11px] text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-50"
            title={t('cost.refreshBalance')}
          >
            <Icon name="refresh" size={12} className={requestLogsLoading ? 'animate-spin' : ''} />
            {t('cost.refresh')}
          </button>
        </div>
      </div>
      <div className="modern-card rounded-lg overflow-hidden mb-6">
        {(() => {
          // 前端过滤：status_code 2xx 视为 success；非 2xx 或有 error_message 视为 error
          const filtered = requestLogs.filter((r) => {
            if (logStatusFilter === 'all') return true
            const codeOk = r.status_code != null && r.status_code >= 200 && r.status_code < 300
            const isError = !codeOk || (r.error_message != null && r.error_message.length > 0)
            return logStatusFilter === 'success' ? !isError : isError
          })
          if (requestLogsLoading && requestLogs.length === 0) {
            return <p className="px-4 py-8 text-center text-sm text-[var(--text-secondary)]">{t('common.loading')}</p>
          }
          if (filtered.length === 0) {
            return <p className="px-4 py-8 text-center text-sm text-[var(--text-secondary)]">{t('cost.requestLogsEmpty')}</p>
          }
          return (
            <>
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-[var(--border)] text-[var(--text-secondary)]">
                    <th className="text-left px-3 py-2">{t('cost.time')}</th>
                    <th className="text-left px-3 py-2">{t('cost.status')}</th>
                    <th className="text-left px-3 py-2">{t('cost.model')}</th>
                    <th className="text-right px-3 py-2 tnum">{t('cost.input')}</th>
                    <th className="text-right px-3 py-2 tnum">{t('cost.output')}</th>
                    <th className="text-right px-3 py-2 tnum">{t('cost.cacheRead')}</th>
                    <th className="text-right px-3 py-2 tnum">{t('cost.ttfb')}</th>
                    <th className="text-right px-3 py-2 tnum">{t('cost.latency')}</th>
                    <th className="text-right px-3 py-2 tnum">{t('cost.fee')}</th>
                    <th className="text-left px-3 py-2 max-w-[200px]">{t('cost.error')}</th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.map((r) => {
                    const codeOk = r.status_code != null && r.status_code >= 200 && r.status_code < 300
                    const isError = !codeOk || (r.error_message != null && r.error_message.length > 0)
                    const statusTone = isError ? 'var(--danger)' : r.is_streaming ? 'var(--accent)' : 'var(--success)'
                    return (
                      <tr key={r.id} className="border-b border-[var(--border)] last:border-0 align-top">
                        <td className="px-3 py-2 whitespace-nowrap">{formatDateTime(r.created_at)}</td>
                        <td className="px-3 py-2">
                          <span
                            className="px-1.5 py-0.5 rounded text-[10.5px] font-mono"
                            style={{ background: statusTone, color: '#fff', opacity: isError ? 1 : 0.85 }}
                            title={r.status_code != null ? `HTTP ${r.status_code}` : t('cost.noStatusCode')}
                          >
                            {r.status_code ?? (r.error_message ? 'ERR' : '—')}
                          </span>
                        </td>
                        <td className="px-3 py-2 max-w-[160px] truncate" title={r.model ?? ''}>
                          {r.model ?? '—'}
                        </td>
                        <td className="px-3 py-2 text-right tnum">{formatTokens(r.input_tokens)}</td>
                        <td className="px-3 py-2 text-right tnum">{formatTokens(r.output_tokens)}</td>
                        <td className="px-3 py-2 text-right tnum">
                          {r.cache_read_tokens > 0 ? (
                            <span className="text-[var(--success)]">{formatTokens(r.cache_read_tokens)}</span>
                          ) : (
                            <span className="text-[var(--text-muted)]">—</span>
                          )}
                        </td>
                        <td className="px-3 py-2 text-right tnum">
                          {r.first_token_ms != null ? `${(r.first_token_ms / 1000).toFixed(2)}s` : '—'}
                        </td>
                        <td className="px-3 py-2 text-right tnum">
                          {r.latency_ms != null ? `${(r.latency_ms / 1000).toFixed(2)}s` : '—'}
                        </td>
                        <td className="px-3 py-2 text-right tnum whitespace-nowrap">
                          ¥{r.total_cost_cny.toFixed(4)}
                        </td>
                        <td className="px-3 py-2 max-w-[200px]">
                          {r.error_message ? (
                            <span
                              className="text-[10.5px] text-[var(--danger)] line-clamp-2"
                              title={r.error_message}
                            >
                              {r.error_message}
                            </span>
                          ) : (
                            <span className="text-[var(--text-muted)] text-[10.5px]">—</span>
                          )}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
              {/* 分页：← / → + 当前页 / 是否有下一页 */}
              <div className="flex items-center justify-between px-3 py-2 border-t border-[var(--border)] text-[11px] text-[var(--text-muted)] tnum">
                <span>{t('cost.page', { page: logPage + 1 })}</span>
                <div className="flex items-center gap-2">
                  <button
                    onClick={() => setLogPage((p) => Math.max(0, p - 1))}
                    disabled={logPage === 0 || requestLogsLoading}
                    className="px-2 py-0.5 rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                  >
                    ←
                  </button>
                  <button
                    onClick={() => setLogPage((p) => p + 1)}
                    disabled={requestLogs.length < LOG_PAGE_SIZE || requestLogsLoading}
                    className="px-2 py-0.5 rounded-md border border-[var(--border)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                  >
                    →
                  </button>
                </div>
              </div>
            </>
          )
        })()}
      </div>

      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('cost.daily')}</h3>
      <div className="modern-card rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-[var(--border)] text-[var(--text-secondary)]">
              <th className="text-left px-4 py-2">{t('cost.date')}</th>
              <th className="text-right px-4 py-2 tnum">{t('cost.requests')}</th>
              <th className="text-right px-4 py-2 tnum">{t('cost.input')}</th>
              <th className="text-right px-4 py-2 tnum">{t('cost.output')}</th>
              <th className="text-right px-4 py-2 tnum">{t('cost.fee')}</th>
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
    <div className="modern-card rounded-lg p-4">
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
    <div className="modern-card p-4 transition-all hover:border-[var(--border-strong)]">
      <p className="text-xs text-[var(--text-secondary)]">{label}</p>
      <p className="text-lg font-semibold mt-1 tnum" style={color ? { color } : undefined}>{value}</p>
    </div>
  )
}

function ReliabilityValue({ label, value }: { label: string; value: string }) {
  return <div><p className="text-xs text-[var(--text-secondary)] mb-1">{label}</p><p className="tnum">{value}</p></div>
}

function rateTone(value?: number): 'ok' | 'warn' | 'bad' {
  return (value ?? 0) >= 0.95 ? 'ok' : (value ?? 0) >= 0.8 ? 'warn' : 'bad'
}

function scoreTone(value?: number): 'ok' | 'warn' | 'bad' {
  return (value ?? 0) >= 90 ? 'ok' : (value ?? 0) >= 75 ? 'warn' : 'bad'
}

/** 预算进度卡：日/月预算已用 vs 上限；上限为 null 时显示"未设上限" */
function BudgetMeter({ label, used, limit }: { label: string; used: number; limit: number | null }) {
  const { t } = useTranslation()
  const hasLimit = limit != null && limit > 0
  const pct = hasLimit ? Math.min(100, (used / (limit as number)) * 100) : 0
  // 颜色随占比：<60 绿、60-85 黄、>85 红
  const level = !hasLimit ? 'normal' : pct >= 85 ? 'danger' : pct >= 60 ? 'warn' : 'normal'
  const remaining = hasLimit ? Math.max(0, (limit as number) - used) : null
  return (
    <div className="modern-card p-4">
      <div className="flex items-center justify-between mb-1.5">
        <p className="text-xs text-[var(--text-secondary)]">{label}</p>
        {hasLimit ? (
          <span
            className={`badge-tone ${level === 'danger' ? 'badge-tone-bad' : level === 'warn' ? 'badge-tone-warn' : 'badge-tone-ok'}`}
            title={t('cost.budgetUsedOf', { used: used.toFixed(2), limit: (limit as number).toFixed(2) })}
          >
            {pct.toFixed(0)}%
          </span>
        ) : (
          <span className="badge-tone badge-tone-info">{t('cost.budgetUnlimited')}</span>
        )}
      </div>
      <p className="text-lg font-semibold tnum">
        ¥{used.toFixed(2)}
        {hasLimit && <span className="text-xs text-[var(--text-muted)] font-normal ml-2">/ ¥{(limit as number).toFixed(2)}</span>}
      </p>
      <div className="context-meter mt-2.5" style={{ height: 3 }}>
        <div
          className="context-meter-fill"
          data-level={level}
          style={{ width: hasLimit ? `${Math.max(pct, 1)}%` : '100%', opacity: hasLimit ? 1 : 0.3 }}
        />
      </div>
      <p className="text-[11px] text-[var(--text-muted)] mt-1.5 tnum">
        {hasLimit
          ? t('cost.budgetRemaining', { amount: remaining!.toFixed(2) })
          : t('cost.budgetUnlimitedHint')}
      </p>
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
