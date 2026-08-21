import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import type { TaskPlan, ToolRun, AgentRun } from '../../stores/projectStore'
import type { ChatOptions } from '../../api/project'
import type { ProviderModel } from '../../api/provider'
import Icon from '../../icons/Icon'
import { ToolRunRow, AgentRunCard } from './toolRuns'
import { getRequestLogs, type RequestLog } from '../../api/cost'

/* ============ 分支选择器（顶部栏） ============ */
export function BranchSelector({
  current,
  branches,
  onSwitch,
}: {
  current: string | null
  branches: string[]
  onSwitch: (branch: string) => Promise<string | null>
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const ref = useRef<HTMLDivElement>(null)

  // 外部点击关闭
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [])

  const pick = async (b: string) => {
    if (b === current || busy) return
    setBusy(true)
    setErr(null)
    const e = await onSwitch(b)
    setBusy(false)
    if (e) setErr(e)
    else setOpen(false)
  }

  return (
    <div className="relative shrink-0" ref={ref}>
      <button
        onClick={() => setOpen((v) => !v)}
        title={t('home.switchBranch')}
        className={`flex items-center gap-1.5 pl-2 pr-1.5 py-1 rounded-lg text-[11px] transition-colors ${
          open
            ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
            : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
        }`}
      >
        <Icon name="git-branch" size={12} />
        <span className="max-w-28 truncate font-mono">{busy ? t('home.switchingBranch') : (current ?? '—')}</span>
        {busy ? (
          <span className="w-2.5 h-2.5 rounded-full border border-[var(--accent)] border-t-transparent animate-spin" />
        ) : (
          <Icon name="chevron-right" size={11} className="rotate-90 opacity-60" />
        )}
      </button>
      {open && (
        <div className="absolute left-0 top-full mt-1.5 w-60 max-h-72 overflow-y-auto rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 animate-modal-in">
          {err && (
            <div className="px-3 py-2 text-[11px] text-[var(--danger)] break-all border-b border-[var(--border)] whitespace-pre-wrap">
              {err}
            </div>
          )}
          <div className="px-3 py-1.5 text-[10px] font-medium text-[var(--text-muted)]">{t('home.branchList')}</div>
          {branches.map((b) => (
            <button
              key={b}
              onClick={() => pick(b)}
              disabled={busy}
              className={`w-full flex items-center gap-2 px-3 py-1.5 text-[12px] text-left transition-colors ${
                b === current
                  ? 'text-[var(--accent)] font-medium'
                  : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
              } disabled:opacity-50`}
            >
              {b === current && <Icon name="check" size={12} className="shrink-0" />}
              {b !== current && <span className="w-3 shrink-0" />}
              <span className="flex-1 truncate font-mono">{b}</span>
              {b === current && <span className="text-[10px] opacity-60">{t('home.branchCurrent')}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

/* ============ 模型设置弹层（对话框内切换模型 / 代理 / 采样参数） ============ */
export function ModelSettingsPopover({
  catalog,
  options,
  onChange,
}: {
  catalog: { providerName: string; models: ProviderModel[] }[]
  options: ChatOptions
  onChange: (next: ChatOptions) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const totalModels = catalog.reduce((n, g) => n + g.models.length, 0)
  // 模型推荐：打开弹层时拉最近 200 条 request_logs，按 model 分组打分（成功率 + 平均成本 + 平均延迟）
  // 打分公式：success_rate * 0.6 + cost_efficiency * 0.3 + speed_score * 0.1
  // - cost_efficiency = min(1, 0.001 / avg_cost_cny)（¥0.001 = 满分基准，贵的递减）
  // - speed_score = min(1, 3000 / avg_latency_ms)（3 秒内 = 满分基准，慢的递减）
  const [recLogs, setRecLogs] = useState<RequestLog[]>([])
  const [recLoading, setRecLoading] = useState(false)
  useEffect(() => {
    let cancelled = false
    setRecLoading(true)
    getRequestLogs({ limit: 200, offset: 0 })
      .then((logs) => !cancelled && setRecLogs(logs))
      .catch(() => !cancelled && setRecLogs([]))
      .finally(() => !cancelled && setRecLoading(false))
    return () => { cancelled = true }
  }, [])
  const recommendation = useMemo(() => {
    if (recLogs.length === 0) return null
    // 按 model 分组
    const byModel = new Map<string, { ok: number; total: number; costSum: number; latSum: number; latCount: number }>()
    for (const r of recLogs) {
      const m = r.model ?? '(unknown)'
      const cur = byModel.get(m) ?? { ok: 0, total: 0, costSum: 0, latSum: 0, latCount: 0 }
      cur.total++
      const codeOk = r.status_code != null && r.status_code >= 200 && r.status_code < 300
      if (codeOk && !(r.error_message && r.error_message.length > 0)) cur.ok++
      cur.costSum += r.total_cost_cny
      if (r.latency_ms != null) { cur.latSum += r.latency_ms; cur.latCount++ }
      byModel.set(m, cur)
    }
    // 打分
    const scored = Array.from(byModel.entries())
      .filter(([, s]) => s.total >= 3) // 至少 3 次调用才进推荐
      .map(([model, s]) => {
        const successRate = s.ok / s.total
        const avgCost = s.costSum / s.total
        const avgLat = s.latCount > 0 ? s.latSum / s.latCount : 99999
        const costEff = Math.min(1, 0.001 / Math.max(avgCost, 0.0001))
        const speed = Math.min(1, 3000 / Math.max(avgLat, 100))
        const score = successRate * 0.6 + costEff * 0.3 + speed * 0.1
        return { model, score, successRate, avgCost, avgLat, samples: s.total }
      })
      .sort((a, b) => b.score - a.score)
    if (scored.length === 0) return null
    return scored
  }, [recLogs])

  const setNum = (key: 'temperature' | 'top_p' | 'max_tokens', raw: string) => {
    const v = raw.trim()
    onChange({ ...options, [key]: v === '' ? undefined : Number(v) })
  }

  const proxyState = options.use_proxy === true ? 'on' : 'off'

  return (
    <div className="absolute bottom-full left-0 mb-2 w-[360px] rounded-xl modern-card shadow-2xl shadow-black/40 z-50 animate-modal-in overflow-hidden">
      <div className="px-3 py-2 text-[11px] font-medium text-[var(--text-muted)] border-b border-[var(--border)]">
        {t('home.modelSettings')}
      </div>
      <div className="p-3 space-y-3.5 max-h-[55vh] overflow-y-auto">
        {/* 模型推荐：基于历史 request_logs 计算成功率/成本/延迟综合分数
         * - 拉最近 200 条：足够统计可信度，又不会让弹层等太久
         * - 推荐模型若在 catalog 中 → 显示"应用"按钮；不在 → 仅显示"未配置"提示 */}
        {recommendation && recommendation.length > 0 && (
          <div className="rounded-lg bg-[var(--accent)]/5 border border-[var(--accent)]/20 px-2.5 py-2">
            <div className="flex items-center gap-1.5 mb-1.5">
              <Icon name="lightbulb" size={11} className="text-[var(--accent)]" />
              <span className="text-[10.5px] font-medium text-[var(--accent)]">{t('home.modelRec')}</span>
            </div>
            {(() => {
              const top = recommendation[0]
              // 查 catalog：找到 model.id 等于该 model 名的项
              const found = catalog.find((g) => g.models.some((m) => m.model_id === top.model || m.id === top.model))
              const inUse = options.model_id === (found?.models.find((m) => m.model_id === top.model || m.id === top.model)?.id)
              return (
                <div className="flex items-center justify-between gap-2">
                  <div className="min-w-0 flex-1">
                    <div className="text-[12px] font-medium truncate" title={top.model}>
                      {top.model}
                    </div>
                    <div className="text-[10px] text-[var(--text-muted)] tnum mt-0.5 flex items-center gap-2 flex-wrap">
                      <span>{(top.successRate * 100).toFixed(0)}% ✓</span>
                      <span>¥{top.avgCost.toFixed(4)}/次</span>
                      {top.avgLat < 99999 && <span>{(top.avgLat / 1000).toFixed(1)}s</span>}
                      <span>· {top.samples} 次样本</span>
                    </div>
                  </div>
                  {found ? (
                    inUse ? (
                      <span className="text-[10px] px-2 py-0.5 rounded-md bg-[var(--success)]/15 text-[var(--success)] font-medium shrink-0">
                        {t('home.modelRecInUse')}
                      </span>
                    ) : (
                      <button
                        onClick={() => {
                          const m = found.models.find((m) => m.model_id === top.model || m.id === top.model)
                          if (m) onChange({ ...options, model_id: m.id })
                        }}
                        className="h-6 px-2 rounded-md btn-primary text-[10.5px] font-medium active:scale-[0.98] shrink-0"
                      >
                        {t('home.modelRecApply')}
                      </button>
                    )
                  ) : (
                    <span className="text-[10px] text-[var(--text-muted)] shrink-0">{t('home.modelRecNotConfigured')}</span>
                  )}
                </div>
              )
            })()}
          </div>
        )}
        {recLoading && recommendation === null && (
          <div className="text-[10.5px] text-[var(--text-muted)] tnum">{t('home.modelRecLoading')}</div>
        )}

        {/* 模型选择 */}
        <div>
          <div className="text-[10px] font-medium text-[var(--text-muted)] mb-1.5">{t('home.currentModel')}</div>
          {totalModels === 0 ? (
            <div className="rounded-lg border border-dashed border-[var(--border)] px-3 py-3 text-[11px] text-[var(--text-muted)] text-center leading-relaxed">
              {t('home.noModels')}
              <button
                onClick={() => navigate('/providers')}
                className="block mx-auto mt-2 px-3 py-1 rounded-md btn-primary text-[11px] font-medium transition-colors"
              >
                {t('home.goProviders')}
              </button>
            </div>
          ) : (
            <select
              value={options.model_id ?? ''}
              onChange={(e) => onChange({ ...options, model_id: e.target.value || undefined })}
              className="w-full h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
            >
              <option value="">{t('provider.modelDefault')}</option>
              {catalog.map((g) => (
                <optgroup key={g.providerName} label={g.providerName}>
                  {g.models.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.display_name ?? m.model_id}
                      {m.is_default ? ' ★' : ''}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          )}
        </div>

        {/* 代理开关（二态：开 / 关，默认关） */}
        <div className="flex items-center justify-between gap-3">
          <span className="text-[12px] text-[var(--text-secondary)]">{t('home.proxyForChat')}</span>
          <div className="flex rounded-lg border border-[var(--border)] overflow-hidden shrink-0">
            {(
              [
                ['on', t('home.proxyOn')],
                ['off', t('home.proxyOff')],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                onClick={() => onChange({ ...options, use_proxy: key === 'on' })}
                className={`px-2.5 py-1 text-[11px] transition-colors ${
                  proxyState === key
                    ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                    : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                }`}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        {/* 协议端点选择（Provider 配置多端点时生效，如 DeepSeek 的 OpenAI/Anthropic） */}
        <div>
          <div className="text-[10px] font-medium text-[var(--text-muted)] mb-1.5">{t('home.protocolEndpoint')}</div>
          <select
            value={options.protocol ?? ''}
            onChange={(e) => onChange({ ...options, protocol: e.target.value || undefined })}
            className="w-full h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
          >
            <option value="">{t('home.protocolDefault')}</option>
            <option value="openai">{t('provider.protoOpenai')}</option>
            <option value="anthropic">{t('provider.protoAnthropic')}</option>
            <option value="gemini">{t('provider.protoGemini')}</option>
          </select>
          <p className="text-[10px] text-[var(--text-muted)] mt-1 leading-snug">{t('home.protocolHint')}</p>
        </div>

        {/* 采样参数 */}
        <div className="grid grid-cols-3 gap-2">
          <NumField
            label={t('home.temperature')}
            value={options.temperature}
            onChange={(v) => setNum('temperature', v)}
            placeholder="0.7"
          />
          <NumField
            label={t('home.topP')}
            value={options.top_p}
            onChange={(v) => setNum('top_p', v)}
            placeholder="1.0"
          />
          <NumField
            label={t('home.maxTokens')}
            value={options.max_tokens}
            onChange={(v) => setNum('max_tokens', v)}
            placeholder="4096"
          />
        </div>

        {/* 推理深度（部分推理模型支持，OpenAI 兼容协议） */}
        <div>
          <div className="text-[10px] font-medium text-[var(--text-muted)] mb-1.5">{t('home.reasoningEffort')}</div>
          <select
            value={options.reasoning_effort ?? ''}
            onChange={(e) => onChange({ ...options, reasoning_effort: e.target.value || undefined })}
            className="w-full h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
          >
            <option value="">{t('home.reasoningDefault')}</option>
            <option value="low">{t('home.reasoningLow')}</option>
            <option value="medium">{t('home.reasoningMedium')}</option>
            <option value="high">{t('home.reasoningHigh')}</option>
          </select>
          <p className="text-[10px] text-[var(--text-muted)] mt-1 leading-snug">{t('home.reasoningHint')}</p>
        </div>

        {/* 子 Agent 设置（Claude Code subagent / ArkClaw 多 Agent） */}
        <div className="pt-3 border-t border-[var(--border)]">
          {/* 第一行：文字（标签）——「子Agent默认模型」与「最大并发数」同行 */}
          <div className="flex items-center justify-between gap-2 mb-1.5">
            <div className="text-[10px] font-medium text-[var(--text-muted)]">{t('home.subAgentModel')}</div>
            <div className="flex items-center gap-1.5 shrink-0">
              <span className="text-[10px] font-medium text-[var(--text-muted)] whitespace-nowrap">
                {t('home.maxConcurrency')}
              </span>
            </div>
          </div>
          {/* 第二行：输入框——默认模型 select 与并发数输入同行 */}
          <div className="flex items-center gap-2">
            <select
              value={options.sub_model_id ?? ''}
              onChange={(e) => onChange({ ...options, sub_model_id: e.target.value || undefined })}
              className="flex-1 min-w-0 h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
            >
              <option value="">{t('home.subAgentFollowMain')}</option>
              {catalog.map((g) => (
                <optgroup key={g.providerName} label={g.providerName}>
                  {g.models.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.display_name ?? m.model_id}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
            <input
              type="number"
              min={1}
              max={16}
              value={options.max_concurrency ?? ''}
              placeholder="3"
              onChange={(e) => {
                const v = e.target.value
                const n = Number(v)
                onChange({
                  ...options,
                  max_concurrency:
                    v.trim() === '' || !Number.isFinite(n)
                      ? undefined
                      : Math.max(1, Math.min(16, Math.round(n))),
                })
              }}
              className="w-14 h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[12px] tabular-nums outline-none placeholder:text-[var(--text-muted)]/50 focus:border-[var(--accent)] transition-colors"
            />
          </div>
          <p className="text-[10px] text-[var(--text-muted)] leading-relaxed mt-1.5">{t('home.subAgentHint')}</p>
        </div>

        {/* 工具权限：分级审核（只确认危险操作） / 自动审核（逐次确认） / 完全放任（直接执行） */}
        <div className="pt-3 border-t border-[var(--border)]">
          <div className="text-[10px] font-medium text-[var(--text-muted)] mb-1.5">{t('home.toolApproval')}</div>
          <select
            value={options.tool_approval ?? 'auto'}
            onChange={(e) =>
              onChange({
                ...options,
                tool_approval: e.target.value === 'auto' ? undefined : (e.target.value as 'ask' | 'auto' | 'first_write' | 'allow_all'),
              })
            }
            className="w-full h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
          >
            <option value="allow_all">{t('home.toolApprovalAllowAll')}</option>
            <option value="auto">{t('home.toolApprovalAuto')}</option>
            <option value="first_write">{t('home.toolApprovalFirstWrite')}</option>
            <option value="ask">{t('home.toolApprovalAsk')}</option>
          </select>
          <p className="text-[10px] text-[var(--text-muted)] mt-1 leading-snug">{t('home.toolApprovalHint')}</p>
        </div>

        {/* 计划/审查模式：Agent 先出计划，用户确认后再执行 */}
        <div className="pt-3 border-t border-[var(--border)]">
          <button
            type="button"
            onClick={() => onChange({ ...options, plan_mode: options.plan_mode ? undefined : true })}
            className="w-full flex items-center justify-between gap-2 group"
          >
            <div className="text-left min-w-0">
              <div className="text-[10px] font-medium text-[var(--text-muted)] group-hover:text-[var(--text-secondary)] transition-colors">
                {t('home.planMode')}
              </div>
              <p className="text-[10px] text-[var(--text-muted)] mt-0.5 leading-snug">{t('home.planModeHint')}</p>
            </div>
            <span
              className={`shrink-0 w-9 h-5 rounded-full relative transition-colors ${
                options.plan_mode ? 'bg-[var(--accent)]' : 'bg-[var(--border)]'
              }`}
            >
              <span
                className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white shadow transition-transform ${
                  options.plan_mode ? 'translate-x-4' : ''
                }`}
              />
            </span>
          </button>
        </div>

        {/* 原生工具调用（function calling）：OpenAI 兼容协议注入 tools，模型结构化返回工具调用（与文本标记并行） */}
        <div className="pt-3 border-t border-[var(--border)]">
          <button
            type="button"
            onClick={() => onChange({ ...options, native_tools: options.native_tools ? undefined : true })}
            className="w-full flex items-center justify-between gap-2 group"
          >
            <div className="text-left min-w-0">
              <div className="text-[10px] font-medium text-[var(--text-muted)] group-hover:text-[var(--text-secondary)] transition-colors">
                {t('home.nativeTools')}
              </div>
              <p className="text-[10px] text-[var(--text-muted)] mt-0.5 leading-snug">{t('home.nativeToolsHint')}</p>
            </div>
            <span
              className={`shrink-0 w-9 h-5 rounded-full relative transition-colors ${
                options.native_tools ? 'bg-[var(--accent)]' : 'bg-[var(--border)]'
              }`}
            >
              <span
                className={`absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white shadow transition-transform ${
                  options.native_tools ? 'translate-x-4' : ''
                }`}
              />
            </span>
          </button>
        </div>

        {/* 排队消息提交方式：逐个提交（缺省） / 一起提交（合并全部排队消息为一条） */}
        <div className="pt-3 border-t border-[var(--border)]">
          <div className="text-[10px] font-medium text-[var(--text-muted)] mb-1.5">{t('home.batchQueued')}</div>
          <div className="flex rounded-lg border border-[var(--border)] overflow-hidden">
            {(
              [
                ['one', t('home.batchQueuedOne')],
                ['all', t('home.batchQueuedAll')],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                onClick={() =>
                  onChange({ ...options, batch_queued: key === 'all' ? true : undefined })
                }
                className={`flex-1 px-2 py-1.5 text-[11px] transition-colors ${
                  (options.batch_queued === true) === (key === 'all')
                    ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                    : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                }`}
              >
                {label}
              </button>
            ))}
          </div>
          <p className="text-[10px] text-[var(--text-muted)] mt-1 leading-snug">{t('home.batchQueuedHint')}</p>
        </div>

        <p className="text-[10px] text-[var(--text-muted)] leading-relaxed">{t('home.chatModelHint')}</p>
      </div>
    </div>
  )
}

/* ============ 采样参数数字输入 ============ */
export function NumField({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string
  value: number | undefined
  onChange: (raw: string) => void
  placeholder?: string
}) {
  return (
    <label className="block min-w-0">
      <span className="block text-[10px] font-medium text-[var(--text-muted)] mb-1 truncate" title={label}>
        {label}
      </span>
      <input
        type="number"
        step="any"
        value={value ?? ''}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        className="w-full h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[12px] tabular-nums outline-none placeholder:text-[var(--text-muted)]/50 focus:border-[var(--accent)] transition-colors"
      />
    </label>
  )
}

/* ============ 任务进度清单（计划卡）：Agent 计划列表 + 工具执行联动，任务结束保留展示 ============ */
export const PlanCard = memo(function PlanCard({ plan }: { plan: TaskPlan }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(true)
  const doneCount = plan.steps.filter((s) => s.status === 'done').length
  const errCount = plan.steps.filter((s) => s.status === 'error').length
  const running = plan.phase === 'running'
  const total = plan.steps.length
  const statusLabel = plan.phase === 'error' ? t('home.planFailed') : running ? t('home.planRunning') : t('home.planDone')
  const statusColor = plan.phase === 'error' ? 'text-[var(--danger)]' : running ? 'text-[var(--accent)]' : 'text-[var(--success)]'

  return (
    <div className="overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 py-1.5 text-left hover:opacity-80 transition-opacity"
        title={t('home.planToggle')}
      >
        <Icon
          name="check"
          size={11}
          className={plan.phase === 'error' ? 'text-[var(--danger)]' : running ? 'text-[var(--accent)]' : 'text-[var(--success)]'}
        />
        <div className="flex-1 min-w-0">
          <span className="text-[12px] text-[var(--text-secondary)]">{t('home.planTitle')}</span>
          <span className="text-[11px] text-[var(--text-muted)] ml-2">
            {doneCount}/{total}
            {errCount > 0 && <span className="text-[var(--danger)] ml-1">· {errCount}×</span>}
          </span>
        </div>
        <span className={`text-[11px] shrink-0 flex items-center gap-1 ${statusColor}`}>
          {running && (
            <span className="inline-block w-2.5 h-2.5 rounded-full border border-[var(--accent)] border-t-transparent animate-spin align-middle" />
          )}
          {statusLabel}
        </span>
        <Icon name="chevron-right" size={11} className={`text-[var(--text-muted)] transition-transform ${open ? 'rotate-90' : ''}`} />
      </button>
      {open && (
        <ol className="border-t border-[var(--border)]/60 py-1.5 space-y-0.5">
          {plan.steps.map((s, i) => (
            <li key={i} className="flex items-start gap-2 py-0.5">
              <span
                className={`w-4 h-4 rounded-full flex items-center justify-center shrink-0 mt-px text-[9px] font-semibold ${
                  s.status === 'done'
                    ? 'text-[var(--success)]'
                    : s.status === 'error'
                      ? 'text-[var(--danger)]'
                      : s.status === 'running'
                        ? 'text-[var(--accent)]'
                        : 'text-[var(--text-muted)]'
                }`}
              >
                {s.status === 'done' ? (
                  <Icon name="check" size={10} />
                ) : s.status === 'error' ? (
                  <Icon name="close" size={10} />
                ) : s.status === 'running' ? (
                  <span className="w-2.5 h-2.5 rounded-full border border-[var(--accent)] border-t-transparent animate-spin" />
                ) : (
                  i + 1
                )}
              </span>
              <span
                className={`flex-1 min-w-0 text-[12px] leading-relaxed ${
                  s.status === 'pending' ? 'text-[var(--text-muted)]' : 'text-[var(--text-secondary)]'
                }`}
              >
                {s.text}
              </span>
            </li>
          ))}
        </ol>
      )}
    </div>
  )
})

/* ============ 任务过程徽章（ChatGPT 式）：中间所有过程折叠为一行，点击展开明细 ============ */
/** 任务过程徽章：流式时显示“已处理 N 个操作中 · mm:ss”，完成后显示“已处理 N 个操作”；
 * 展开后展示全部工具明细（ToolRunRow）+ 子 Agent 卡片，对话流保持干净不中断。 */
export const TaskOpsBadge = memo(function TaskOpsBadge({
  running,
  count,
  time,
  toolName,
  open,
  onToggle,
  runs,
  agents,
}: {
  running?: boolean
  count: number
  time?: string
  toolName?: string
  open: boolean
  onToggle: () => void
  runs: ToolRun[]
  agents: AgentRun[]
}) {
  const { t } = useTranslation()
  const hasDetail = runs.length > 0 || agents.length > 0
  return (
    <div className="overflow-hidden">
      <button
        type="button"
        onClick={onToggle}
        className={`w-full flex items-center gap-2 py-1.5 text-left transition-opacity ${hasDetail ? 'hover:opacity-80' : ''}`}
        title={t('home.toggleTaskOps')}
      >
        {running ? (
          <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] animate-pulse shrink-0" />
        ) : (
          <Icon name="check" size={11} className="text-[var(--success)] shrink-0" />
        )}
        <span className="text-[var(--text-muted)] text-[11px] tabular-nums shrink-0">
          {running && count === 0
            ? t('home.taskProcessing', { time })
            : running
              ? t('home.taskOpsProcessing', { count, time })
              : t('home.taskOpsDone', { count })}
        </span>
        {toolName && <span className="text-[var(--text-muted)] truncate font-mono text-[11px]">· {toolName}</span>}
        {hasDetail && (
          <Icon
            name="chevron-right"
            size={11}
            className={`ml-auto text-[var(--text-muted)] transition-transform shrink-0 ${open ? 'rotate-90' : ''}`}
          />
        )}
      </button>
      {open && hasDetail && (
        <div className="border-t border-[var(--border)]/60 divide-y divide-[var(--border)]/50 max-h-80 overflow-y-auto">
          {runs.map((r) => (
            <ToolRunRow key={r.id} run={r} />
          ))}
          {agents.map((r) => (
            <AgentRunCard key={r.id} run={r} />
          ))}
        </div>
      )}
    </div>
  )
})

