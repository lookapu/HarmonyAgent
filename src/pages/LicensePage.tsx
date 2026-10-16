// @ui-states: loading, empty, partial, failed, retry, permission
/**
 * 授权与权益页（P2-2）
 *
 * 回答三个问题：**我现在是什么档位、这一档能不能用某功能、我用超了没**。
 *
 * ## 权益矩阵不在前端
 *
 * 档位 → 能力的对应关系只存在于后端 `services/licensing.rs::TIER_ENTITLEMENTS`。
 * 本页拿到的 `tiers` 是后端算好的结果，前端只负责把 `capability` 键翻成文案。
 *
 * 抄一份到前端的后果是**静默失真**：页面显示 Pro 含某能力，实际点进去被网关拦，
 * 或者反过来（页面说不含，实际能用）。后者更糟——那会让用户以为买错了。
 *
 * ## 这一页刻意不显示「价格」
 *
 * 项目红线是支付暂缓。此处展示的是**分级模型**：每档有什么、当前用量与配额。
 * 写一个价格数字会让用户以为这东西能买，而它现在买不了。
 * 真正的价格与支付入口等收款那一步接上再加，届时本页结构不用改。
 *
 * ## 六态里最容易漏的两个
 *
 * - `permission`：某个能力不可用时，**显示「需要哪一档」而不是只显示「不可用」**。
 *   后者的错误文案里带了所需档位，前端要把它露出来——那正是用户下一步的依据。
 * - `partial`：配额超额不是错误，是「还能用但已超额」。页顶要显式提示，
 *   藏在角落等于没提示。
 */
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from '../components/ui/Button'
import { EmptyState } from '../components/ui/EmptyState'
import {
  getLicenseState,
  listTiers,
  setLicenseTier,
  type Capability,
  type LicenseState,
  type Tier,
  type TierInfo,
} from '../api/license'
import { cn } from '../utils/cn'

/** 权益键 → i18n 前缀。后端少给一个能力，这里就少一行，不补默认值。 */
const CAPABILITY_LABEL: Record<Capability, string> = {
  multi_provider_pool: 'license.capMultiPool',
  team_sharing: 'license.capTeamSharing',
  eval_reports: 'license.capEvalReports',
  lan_collaboration: 'license.capLan',
  audit_trail: 'license.capAudit',
}

const TIER_ORDER: Tier[] = ['free', 'pro', 'team']

export default function LicensePage() {
  const { t } = useTranslation()
  const [state, setState] = useState<LicenseState | null>(null)
  const [tiers, setTiers] = useState<TierInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const load = useCallback(async () => {
    setError(null)
    try {
      const [current, all] = await Promise.all([getLicenseState(), listTiers()])
      setState(current)
      setTiers(all)
    } catch (e) {
      setState(null)
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const changeTier = async (tier: Tier) => {
    if (!state || tier === state.tier) return
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      setState(await setLicenseTier(tier))
      setNotice(t('license.switched', { tier: t(`license.tier_${tier}`) }))
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  const rows = useMemo(() => {
    return (Object.keys(CAPABILITY_LABEL) as Capability[]).map((capability) => ({
      capability,
      labelKey: CAPABILITY_LABEL[capability],
      granted: tiers.some((tier) => tier.capabilities.includes(capability)),
    }))
  }, [tiers])

  if (loading) {
    return (
      <div className="p-4 text-[length:var(--app-text-sm)] text-[var(--text-secondary)]">
        {t('common.loading')}
      </div>
    )
  }

  // 失败态给重试，而不是只显示一句「加载失败」——
  // 读不到授权状态时用户无法判断自己能不能用某功能，这值得一个重试按钮。
  if (!state) {
    return (
      <div className="p-4">
        <EmptyState
          icon="info"
          title={t('license.loadFailed')}
          description={error ?? undefined}
          action={
            <Button onClick={() => {
              setLoading(true)
              void load()
            }}>
              {t('license.retry')}
            </Button>
          }
        />
      </div>
    )
  }

  const { quota, exceeded } = state

  return (
    <div className="space-y-4 p-4">
      <header>
        <h1 className="text-[length:var(--app-text-lg)] font-semibold">
          {t('license.title')}
        </h1>
        <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          {t('license.desc')}
        </p>
      </header>

      {/* 本地切换必须说清楚：这不是买来的。藏起来的话，
          用户看到「Team」会以为公司已经付过钱了。 */}
      <p className="rounded-[length:var(--app-radius-sm)] bg-[var(--bg-primary)] px-2.5 py-2 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
        {t('license.localSwitchNotice')}
      </p>

      {notice && (
        <p className="text-[length:var(--app-text-xs)] text-[var(--success)]">{notice}</p>
      )}
      {error && (
        <p className="text-[length:var(--app-text-xs)] text-[var(--danger)]">{error}</p>
      )}

      {/* 超额提示放页顶。藏在配额表里等于没提示——
          用户看到的是「还能用」，而实际已经超了。 */}
      {exceeded.length > 0 && (
        <div className="rounded-[length:var(--app-radius-sm)] border border-[var(--warning)] bg-[var(--bg-primary)] p-2.5">
          <p className="text-[length:var(--app-text-xs)] font-semibold text-[var(--warning)]">
            {t('license.exceededTitle')}
          </p>
          <ul className="mt-1 space-y-0.5">
            {exceeded.map((breach) => (
              <li
                key={breach.limit}
                className="text-[length:var(--app-text-xs)] text-[var(--text-secondary)]"
              >
                {t(`license.quota_${breach.limit}`)}: {breach.used} / {breach.cap}
              </li>
            ))}
          </ul>
        </div>
      )}

      <section className="modern-card p-3">
        <h2 className="text-[length:var(--app-text-md)] font-semibold">
          {t('license.currentSection')}
        </h2>
        <p className="mt-1 text-[length:var(--app-text-sm)]">
          {t('license.currentTier')}:{' '}
          <span className="font-semibold text-[var(--text-primary)]">
            {t(`license.tier_${state.tier}`)}
          </span>
        </p>
        <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
          {t('license.quotaPeriod', { period: quota.period })}
        </p>

        {/* 用量为 0 是真实的「还没用」，不是数据缺失——
            两者在界面上长得一样，用户会以为统计坏了。 */}
        {quota.runs === 0 && quota.tool_calls === 0 && quota.cost_cny === 0 ? (
          <div className="mt-2">
            <EmptyState compact title={t('license.noUsage')} />
          </div>
        ) : (
          <dl className="mt-2 grid grid-cols-3 gap-2">
            <QuotaCell label={t('license.quotaRuns')} used={quota.runs} />
            <QuotaCell label={t('license.quotaToolCalls')} used={quota.tool_calls} />
            <QuotaCell label={t('license.quotaCost')} used={quota.cost_cny} />
          </dl>
        )}
      </section>

      <section className="modern-card p-3">
        <h2 className="text-[length:var(--app-text-md)] font-semibold">
          {t('license.compareSection')}
        </h2>
        <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          {t('license.compareDesc')}
        </p>

        <div className="mt-2.5 space-y-2">
          {TIER_ORDER.map((tier) => {
            const info = tiers.find((item) => item.tier === tier)
            const active = state.tier === tier
            return (
              <div
                key={tier}
                className={cn(
                  'rounded-[length:var(--app-radius-sm)] border p-2.5',
                  active
                    ? 'border-[var(--accent)] bg-[var(--bg-primary)]'
                    : 'border-[var(--border)]',
                )}
              >
                <div className="flex items-center justify-between gap-2">
                  <h3 className="text-[length:var(--app-text-sm)] font-semibold">
                    {t(`license.tier_${tier}`)}
                  </h3>
                  {active ? (
                    <span className="text-[length:var(--app-text-xs)] text-[var(--success)]">
                      {t('license.currentBadge')}
                    </span>
                  ) : (
                    <Button
                      size="sm"
                      onClick={() => void changeTier(tier)}
                      loading={busy}
                      disabled={busy}
                    >
                      {t('license.switchTo')}
                    </Button>
                  )}
                </div>
                <p className="mt-1 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
                  {t(`license.tier_${tier}Desc`)}
                </p>
                {info && (
                  <p className="mt-1 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
                    {t('license.tierQuota', {
                      budget:
                        info.monthly_budget_cny === null
                          ? t('license.unlimited')
                          : `${info.monthly_budget_cny} CNY`,
                    })}
                  </p>
                )}
              </div>
            )
          })}
        </div>
      </section>

      <section className="modern-card p-3">
        <h2 className="text-[length:var(--app-text-md)] font-semibold">
          {t('license.matrixSection')}
        </h2>
        <ul className="mt-1.5 space-y-1">
          {rows.map((row) => (
            <li
              key={row.capability}
              className="flex items-center justify-between gap-2 text-[length:var(--app-text-xs)]"
            >
              <span className="text-[var(--text-secondary)]">{t(row.labelKey)}</span>
              {/* 不可用时明写需要哪一档——这是用户下一步的依据，
                  只写「不可用」等于把用户丢在原地。 */}
              <span
                className={cn(
                  'shrink-0',
                  row.granted
                    ? 'text-[var(--success)]'
                    : 'text-[var(--text-muted)]',
                )}
              >
                {row.granted
                  ? t('license.included')
                  : t('license.requiresTier', {
                      tier: t(
                        tiers.find((tier) => tier.capabilities.includes(row.capability))
                          ? 'license.tier_pro'
                          : 'license.tier_team',
                      ),
                    })}
              </span>
            </li>
          ))}
        </ul>
      </section>
    </div>
  )
}

function QuotaCell({ label, used }: { label: string; used: number }) {
  return (
    <div>
      <dt className="text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
        {label}
      </dt>
      <dd className="tnum text-[length:var(--app-text-sm)] font-semibold">
        {used % 1 === 0 ? used : used.toFixed(2)}
      </dd>
    </div>
  )
}
