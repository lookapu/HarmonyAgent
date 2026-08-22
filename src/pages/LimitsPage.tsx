// @ui-states: loading, empty, failed
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { getAgentLimits, setAgentLimits, resetAgentLimits, type AgentLimits } from '../api/limits'

/** 配置项元数据：字段键 / i18n 标签键 / 说明键 / 单位 / 默认值（输入框占位提示） */
interface LimitField {
  key: keyof AgentLimits
  labelKey: string
  descKey: string
  unit?: string
  defaultVal: number
}

/** 默认值（与后端 DEFAULT_* 一致，仅用于占位提示与快速对比） */
const DEFAULT_LIMITS: AgentLimits = {
  tool_call_limit: 300,
  heavy_tool_call_limit: 6,
  repeat_call_limit: 5,
  tool_rounds: 80,
  task_duration_minutes: 30,
  sub_agent_rounds: 20,
  blacklist_fail_threshold: 4,
  repeat_edit_threshold: 3,
  stall_tool_threshold: 10,
  build_fail_converge_threshold: 5,
  goal_reinject_interval: 8,
}

const FIELD_GROUPS: { groupKey: string; fields: LimitField[] }[] = [
  {
    groupKey: 'limits.groupExec',
    fields: [
      { key: 'tool_rounds', labelKey: 'limits.toolRounds', descKey: 'limits.toolRoundsDesc', defaultVal: DEFAULT_LIMITS.tool_rounds },
      { key: 'task_duration_minutes', labelKey: 'limits.taskDuration', descKey: 'limits.taskDurationDesc', unit: 'limits.unitMin', defaultVal: DEFAULT_LIMITS.task_duration_minutes },
      { key: 'tool_call_limit', labelKey: 'limits.toolCalls', descKey: 'limits.toolCallsDesc', defaultVal: DEFAULT_LIMITS.tool_call_limit },
      { key: 'heavy_tool_call_limit', labelKey: 'limits.heavyCalls', descKey: 'limits.heavyCallsDesc', defaultVal: DEFAULT_LIMITS.heavy_tool_call_limit },
      { key: 'sub_agent_rounds', labelKey: 'limits.subRounds', descKey: 'limits.subRoundsDesc', defaultVal: DEFAULT_LIMITS.sub_agent_rounds },
    ],
  },
  {
    groupKey: 'limits.groupGuard',
    fields: [
      { key: 'repeat_call_limit', labelKey: 'limits.repeatCalls', descKey: 'limits.repeatCallsDesc', defaultVal: DEFAULT_LIMITS.repeat_call_limit },
      { key: 'blacklist_fail_threshold', labelKey: 'limits.blacklist', descKey: 'limits.blacklistDesc', defaultVal: DEFAULT_LIMITS.blacklist_fail_threshold },
      { key: 'repeat_edit_threshold', labelKey: 'limits.repeatEdit', descKey: 'limits.repeatEditDesc', defaultVal: DEFAULT_LIMITS.repeat_edit_threshold },
      { key: 'stall_tool_threshold', labelKey: 'limits.stall', descKey: 'limits.stallDesc', defaultVal: DEFAULT_LIMITS.stall_tool_threshold },
      { key: 'build_fail_converge_threshold', labelKey: 'limits.buildFail', descKey: 'limits.buildFailDesc', defaultVal: DEFAULT_LIMITS.build_fail_converge_threshold },
      { key: 'goal_reinject_interval', labelKey: 'limits.goalReinject', descKey: 'limits.goalReinjectDesc', defaultVal: DEFAULT_LIMITS.goal_reinject_interval },
    ],
  },
]

export default function LimitsPage() {
  const { t } = useTranslation()
  const [form, setForm] = useState<Record<keyof AgentLimits, string>>(() => emptyForm())
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  function emptyForm(): Record<keyof AgentLimits, string> {
    const out = {} as Record<keyof AgentLimits, string>
    for (const g of FIELD_GROUPS) {
      for (const f of g.fields) out[f.key] = String(f.defaultVal)
    }
    return out
  }

  const load = async () => {
    try {
      const limits = await getAgentLimits()
      const next = {} as Record<keyof AgentLimits, string>
      for (const g of FIELD_GROUPS) {
        for (const f of g.fields) next[f.key] = String(limits[f.key] ?? f.defaultVal)
      }
      setForm(next)
    } catch (e) {
      setError(t('limits.loadError') + `: ${e}`)
    }
  }

  useEffect(() => {
    load()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps -- 挂载时加载一次

  const handleSave = async () => {
    setError(null)
    const limits = {} as AgentLimits
    for (const g of FIELD_GROUPS) {
      for (const f of g.fields) {
        const v = Number(form[f.key])
        if (!Number.isInteger(v)) {
          setError(t('limits.invalidNumber') + `: ${t(f.labelKey)}`)
          return
        }
        limits[f.key] = v
      }
    }
    setBusy(true)
    try {
      const normalized = await setAgentLimits(limits)
      const next = {} as Record<keyof AgentLimits, string>
      for (const g of FIELD_GROUPS) {
        for (const f of g.fields) next[f.key] = String(normalized[f.key])
      }
      setForm(next)
      setSaved(true)
      setTimeout(() => setSaved(false), 2500)
    } catch (e) {
      setError(t('limits.saveError') + `: ${e}`)
    } finally {
      setBusy(false)
    }
  }

  const handleReset = async () => {
    if (!confirm(t('limits.resetConfirm'))) return
    setBusy(true)
    setError(null)
    try {
      const normalized = await resetAgentLimits()
      const next = {} as Record<keyof AgentLimits, string>
      for (const g of FIELD_GROUPS) {
        for (const f of g.fields) next[f.key] = String(normalized[f.key])
      }
      setForm(next)
      setSaved(true)
      setTimeout(() => setSaved(false), 2500)
    } catch (e) {
      setError(t('limits.saveError') + `: ${e}`)
    } finally {
      setBusy(false)
    }
  }

  const setUnlimited = (key: keyof AgentLimits) => {
    setForm((f) => ({ ...f, [key]: '-1' }))
  }

  return (
    <div className="h-full flex flex-col gap-4">
      <div className="flex items-center justify-between mb-1">
        <div>
          <h2 className="text-xl font-semibold">{t('limits.title')}</h2>
          <p className="text-xs text-[var(--text-secondary)] mt-1">{t('limits.subtitle')}</p>
        </div>
        <div className="flex items-center gap-2">
          {saved && <span className="text-xs text-[var(--success)]">{t('limits.saved')}</span>}
          {error && <span className="text-xs text-[var(--danger)] max-w-[320px] truncate" title={error}>{error}</span>}
          <button
            onClick={handleReset}
            disabled={busy}
            className="px-4 py-2 border border-[var(--border)] text-[var(--text-primary)] rounded-lg text-sm hover:bg-[var(--bg-muted)] transition-colors disabled:opacity-50"
          >
            {t('limits.reset')}
          </button>
          <button onClick={handleSave} disabled={busy} className="px-4 py-2 btn-primary rounded-lg text-sm transition-colors disabled:opacity-50">
            {t('limits.save')}
          </button>
        </div>
      </div>

      {FIELD_GROUPS.map((group) => (
        <div key={group.groupKey} className="border border-[var(--border)] rounded-lg p-4 bg-[var(--bg-secondary)]">
          <h3 className="text-sm font-semibold mb-1">{t(group.groupKey)}</h3>
          <div className="divide-y divide-[var(--border)]">
            {group.fields.map((f) => {
              const value = Number(form[f.key])
              const unlimited = value <= 0
              const dirty = value !== f.defaultVal
              return (
                <div key={f.key} className="py-3 flex items-center gap-4">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-medium text-[var(--text-primary)]">{t(f.labelKey)}</span>
                      {dirty && (
                        <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--accent-soft)] text-[var(--accent)]">
                          {t('limits.modified')}
                        </span>
                      )}
                    </div>
                    <p className="text-xs text-[var(--text-secondary)] mt-0.5">{t(f.descKey)}</p>
                  </div>
                  <div className="flex items-center gap-2 shrink-0">
                    <button
                      onClick={() => setUnlimited(f.key)}
                      title={t('limits.unlimitedTip')}
                      className={`px-2 py-1 rounded-md text-xs border transition-colors ${
                        unlimited
                          ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]'
                          : 'border-[var(--border)] text-[var(--text-secondary)] hover:bg-[var(--bg-muted)]'
                      }`}
                    >
                      {t('limits.unlimited')}
                    </button>
                    <div className="flex items-center gap-1.5">
                      <input
                        type="number"
                        value={form[f.key]}
                        onChange={(e) => setForm((prev) => ({ ...prev, [f.key]: e.target.value }))}
                        spellCheck={false}
                        className="w-24 px-2.5 py-1.5 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-sm text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
                      />
                      {f.unit && <span className="text-xs text-[var(--text-secondary)] whitespace-nowrap">{t(f.unit)}</span>}
                    </div>
                  </div>
                </div>
              )
            })}
          </div>
        </div>
      ))}

      <p className="text-xs text-[var(--text-secondary)] px-1">
        {t('limits.hint')}
      </p>
    </div>
  )
}
