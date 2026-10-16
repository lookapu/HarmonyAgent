// @ui-states: loading, empty, failed, retry
/**
 * 本地使用统计页
 *
 * 回答的问题是「**哪些功能有人在用，哪些是死代码**」——
 * 与成本页（CostPage）回答的「花了多少钱」是两回事，故独立成页。
 *
 * ## 隐私承诺（页面上必须写明，不能只写在文档里）
 *
 * - **默认关闭**，由用户在设置页显式开启
 * - **只落本地 SQLite**，没有任何上传路径；全项目也没有「上报」command
 * - **记录内容**：事件名 + 应用版本 + 少量低基数上下文（页面名/工具名/状态码）
 * - **不记录**：文件路径、文件名、用户名、prompt 内容、API Key
 * - 关闭开关会**连带清空**已记录数据
 */
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  usageStatsSummary,
  usageStatsTotal,
  clearUsageStats,
  usageEventNames,
  recordUsageEventStrict,
  type UsageEventSummary,
} from '../api/telemetry'
import { EVENT_NAMES } from '../utils/telemetry'
import { Button } from '../components/ui/Button'
import { EmptyState } from '../components/ui/EmptyState'

type RangeKey = 'all' | '7' | '30'

export default function UsagePage() {
  const { t } = useTranslation()
  const [range, setRange] = useState<RangeKey>('all')
  const [rows, setRows] = useState<UsageEventSummary[] | null>(null)
  const [total, setTotal] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  /** 自增即重载。与 ConfigPage 同一写法：失败后「重试」改这个值而不是刷新页面 */
  const [reloadToken, setReloadToken] = useState(0)
  const [selfCheck, setSelfCheck] = useState<'idle' | 'ok' | 'fail' | 'drift' | 'notwritten'>('idle')

  // sinceTs 必须在 range 变化时才重算。
  //
  // 写成裸的 `Date.now() - days*86400`（不 memo）时，每次渲染都会算出新值 →
  // `load` 的 useCallback 依赖它 → useEffect 又依赖 load → effect 重跑并重新查库。
  // **实测的严重程度要说准**（我第一版在这里夸大了）：因为 `Math.floor(Date.now()/1000)`
  // 量化到**秒**，只有当两次连续渲染恰好跨过整秒边界时引用才真的变，
  // 所以这不是每帧轮询，而是一串**低频、时机不可预测的重复拉取**——
  // 页面开着不动时它大多会自然停住（没有下一次渲染就没有下一次 effect）。
  // 但它没有任何用途：数据不会因为多查一次而变新，纯粹是白花的 IPC。
  //
  // 另注：默认值 'all' 返回常量 0，所以这个问题**只在用户点过区间筛选后**才存在。
  const sinceTs = useMemo(() => {
    if (range === 'all') return 0
    return Math.floor(Date.now() / 1000) - Number(range) * 86400
  }, [range])

  const load = useCallback(async () => {
    setError(null)
    try {
      const [list, n] = await Promise.all([usageStatsSummary(sinceTs), usageStatsTotal()])
      setRows(list)
      setTotal(n)
    } catch (e) {
      // 读不到就整块不显示：给「0」意味着「确实没有任何使用」，
      // 而实际可能是库还没建好或迁移失败——那是两件完全不同的事
      setRows(null)
      setTotal(null)
      setError(String(e))
    }
  }, [sinceTs])

  useEffect(() => {
    void load()
  }, [load, reloadToken])

  const handleClear = async () => {
    setBusy(true)
    setError(null)
    try {
      await clearUsageStats()
      await load()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  /**
   * 自检：验证「前端事件名与后端白名单没漂移」**并且**「真写进去了」。
   *
   * 这一条不是装饰——埋点漂移（今天 page_view、明天 view_page）是这类系统
   * 最常见的失败模式：后端静默丢弃，界面上看不出任何异常，报表一半对不上。
   *
   * **必须比对 total 的增量，不能只看「没抛错」。** 统计开关关闭时
   * `record_usage_event` 是**静默返回 Ok** 的（没开统计不是错误状态），
   * 于是只判「没抛错」会把「一条都没写进去」显示成「自检通过」——
   * 用户接着去看面板，发现没多出任何行，反而更困惑。
   * 三件事任一为假都会让 total 不涨：白名单对不上、开关没开、迁移没建表。
   * 「阴性结论不可信」在这里的具体形态就是**假通过**。
   */
  const runSelfCheck = async () => {
    setSelfCheck('idle')
    try {
      // 这里排序比较、构建期门禁逐字同序比较，是**故意不同**：
      // 运行时只关心「两边能不能对上」，顺序不同不该对用户报成故障；
      // 顺序一致性属于代码质量，由 scripts/check-telemetry-contract.py 兜。
      const backend = await usageEventNames()
      const frontend = [...EVENT_NAMES].sort()
      const backendSorted = [...backend].sort()
      if (JSON.stringify(frontend) !== JSON.stringify(backendSorted)) {
        setSelfCheck('drift')
        return
      }
      const before = await usageStatsTotal()
      await recordUsageEventStrict('page_view', { selfCheck: true })
      const after = await usageStatsTotal()
      if (after <= before) {
        setSelfCheck('notwritten')
        return
      }
      setSelfCheck('ok')
      await load()
    } catch {
      setSelfCheck('fail')
    }
  }

  const maxCount = rows?.reduce((m, r) => Math.max(m, r.count), 0) ?? 0

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4 pb-10">
      <header>
        <h2 className="text-[length:var(--app-text-lg)] font-semibold">{t('usage.title')}</h2>
        <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          {t('usage.subtitle')}
        </p>
      </header>

      {/* 隐私承诺写在页面上，不只写在文档里 */}
      <section className="modern-card p-4">
        <h3 className="text-[length:var(--app-text-md)] font-semibold">{t('usage.privacyTitle')}</h3>
        <ul className="mt-2 list-disc space-y-1 pl-5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          <li>{t('usage.privacyLocal')}</li>
          <li>{t('usage.privacyNoSensitive')}</li>
          <li>{t('usage.privacyOffClears')}</li>
        </ul>
        <p className="mt-2 text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">
          {t('usage.privacyToggleHint')}
        </p>
      </section>

      <section className="modern-card p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-[length:var(--app-text-md)] font-semibold">{t('usage.eventsTitle')}</h3>
          <div className="flex flex-wrap items-center gap-2">
            <div className="inline-flex shrink-0 items-center gap-0.5 rounded-md border border-[var(--border)] bg-[var(--bg-primary)] p-0.5">
              {(['all', '7', '30'] as const).map((k) => (
                <button
                  key={k}
                  type="button"
                  aria-pressed={range === k}
                  onClick={() => setRange(k)}
                  className={
                    range === k
                      ? 'rounded-[4px] bg-[var(--accent-soft)] px-2.5 py-1 text-[length:var(--app-text-sm)] font-medium text-[var(--accent)]'
                      : 'rounded-[4px] px-2.5 py-1 text-[length:var(--app-text-sm)] font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                  }
                >
                  {t(`usage.range.${k}`)}
                </button>
              ))}
            </div>
            <Button size="sm" icon="refresh" onClick={() => setReloadToken((n) => n + 1)}>
              {t('common.refresh')}
            </Button>
            <Button
              size="sm"
              icon="delete"
              variant="danger"
              loading={busy}
              onClick={() => void handleClear()}
              disabled={busy || total === 0}
            >
              {t('usage.clear')}
            </Button>
          </div>
        </div>

        {error ? (
          <div className="mt-3 space-y-2">
            <p className="text-[length:var(--app-text-xs)] text-[var(--danger)]">
              {t('usage.loadError')}: {error}
            </p>
            <Button size="xs" icon="refresh" onClick={() => setReloadToken((n) => n + 1)}>
              {t('common.retry')}
            </Button>
          </div>
        ) : rows === null ? (
          <p className="mt-3 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
            {t('usage.loading')}
          </p>
        ) : rows.length === 0 ? (
          <EmptyState
            className="mt-3"
            icon="receipt"
            title={t('usage.empty')}
            description={t('usage.emptyHint')}
          />
        ) : (
          <>
            <p className="mt-2 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
              {t('usage.total', { count: total ?? rows.length })}
            </p>
            <ul className="mt-2 flex flex-col gap-1.5">
              {rows.map((r) => (
                <li key={r.event_name} className="flex items-center gap-2">
                  <span className="w-44 shrink-0 truncate text-[length:var(--app-text-xs)]">
                    {t(`usage.event.${r.event_name}`, { defaultValue: r.event_name })}
                  </span>
                  <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-[var(--bg-card)]">
                    <span
                      className="block h-full rounded-full bg-[var(--accent)]"
                      style={{ width: `${maxCount > 0 ? (r.count / maxCount) * 100 : 0}%` }}
                    />
                  </span>
                  <span className="tnum w-12 shrink-0 text-right text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
                    {r.count}
                  </span>
                </li>
              ))}
            </ul>
          </>
        )}

        <div className="mt-4 flex items-center gap-2">
          <Button size="xs" icon="check" onClick={() => void runSelfCheck()}>
            {t('usage.selfCheck')}
          </Button>
          {selfCheck === 'ok' && (
            <span className="text-[length:var(--app-text-xs)] text-[var(--success)]">
              {t('usage.selfCheckOk')}
            </span>
          )}
          {selfCheck === 'fail' && (
            <span className="text-[length:var(--app-text-xs)] text-[var(--danger)]">
              {t('usage.selfCheckFail')}
            </span>
          )}
          {selfCheck === 'drift' && (
            <span className="text-[length:var(--app-text-xs)] text-[var(--warning)]">
              {t('usage.selfCheckDrift')}
            </span>
          )}
          {selfCheck === 'notwritten' && (
            <span className="text-[length:var(--app-text-xs)] text-[var(--warning)]">
              {t('usage.selfCheckNotWritten')}
            </span>
          )}
        </div>
      </section>
    </div>
  )
}
