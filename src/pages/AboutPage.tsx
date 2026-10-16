// @ui-states: loading, failed
/**
 * 关于 / 许可页
 *
 * 回答三个问题：**这是什么、什么许可、里面用了谁的东西**。
 * 第三个问题决定了前两个问题是否可信——一个说不清依赖构成的「合规」声明
 * 在商用分发时是没有意义的。
 *
 * ## 许可数据不写在代码里
 *
 * 清单来自 `src/generated/thirdParty.json`，由 `scripts/gen-license-manifest.py`
 * 从 `Cargo.lock`（经 `cargo metadata`）与 `package-lock.json` + `node_modules`
 * 生成。`scripts/check-licenses.py` 保证它与真源不脱节。
 *
 * 手写这份清单的话，它一定会在下一次 `cargo update` 之后失真，
 * 而失真是**静默**的：页面照常渲染，只是不再完整。
 *
 * ## 为什么按许可分组而不是把每个组件列一行
 *
 * 用户要判断的是「有没有需要我履行义务的东西」，不是背一长串包名。
 * （这里刻意不写具体组件数：它是随依赖变化的，写进注释就是一个会过期的数字。）
 * 非宽松项默认展开，宽松项折叠——但**计数始终可见**，
 * 折叠不能变成「你看不到所以当作没有」。
 */
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { getVersion, getTauriVersion } from '@tauri-apps/api/app'
import { save as dialogSave } from '@tauri-apps/plugin-dialog'
import { writeTextFile } from '@tauri-apps/plugin-fs'
import { Button } from '../components/ui/Button'
import manifest from '../generated/thirdParty.json'

type Tier = 'permissive' | 'notice' | 'copyleft' | 'unknown'

interface Group {
  ecosystem: string
  license: string
  tier: Tier
  nonSpdx: boolean
  count: number
  names: string[]
}

interface Notice {
  ecosystem: string
  name: string
  reason: string
  obligation: string
}

/**
 * 豁免清单**再抄一份**进来是刻意的：页面必须能在离线/只装前端的环境里
 * 展示「为什么这个依赖被放行」，不能靠再调一次 Python。
 * `check-licenses.py` 负责保证它与 `scripts/license-allowlist.json` 一致，
 * 所以这不是「又抄一份」，而是**带门禁的缓存**。
 */
const ALLOWLIST: Notice[] = [
  {
    ecosystem: 'cargo',
    name: 'cssparser / cssparser-macros / dtoa-short / option-ext / selectors',
    reason:
      'MPL-2.0 文件级 copyleft。这些包不是本项目直接选型，而是经 ' +
      'dom_query / dirs-sys → tauri-utils → Tauri 引入的传递依赖。' +
      '除非替换 Tauri 这一技术底座，否则无法移除。',
    obligation:
      '分发时需附带其许可文本；若本项目修改过这些文件，修改后的文件须以 MPL-2.0 开放。' +
      '本项目未修改它们（均为 crates.io 原版）。',
  },
  {
    ecosystem: 'npm',
    name: '@ohos/hvigor',
    reason:
      '华为的包声明「SEE LICENSE IN LICENSE.txt」，但实际随包分发的是 NOTICE.txt' +
      '（包内不存在 LICENSE.txt），因此没有可静态判定的 SPDX 标识。' +
      '该 NOTICE 的内容是 Node.js 18 各组件的版权清单（Intel/BSD、OpenSSL、V8、' +
      'zlib、Chromium/BSD 等，均为宽松许可）。',
    obligation: '商用分发前需人工确认 Huawei 的实际授权条款——此项无法由静态扫描替代。',
  },
]

const TIER_ORDER: Tier[] = ['copyleft', 'unknown', 'notice', 'permissive']

export default function AboutPage() {
  const { t } = useTranslation()
  const [version, setVersion] = useState('')
  const [tauriVersion, setTauriVersion] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [notice, setNotice] = useState<'idle' | 'ok' | 'fail' | 'cancelled'>('idle')

  useEffect(() => {
    let cancelled = false
    // 版本号取不到时**保持空串**而不是编一个：显示一个错的版本号比不显示更糟
    // （用户会拿它去对 bug 报告）
    Promise.all([getVersion(), getTauriVersion()])
      .then(([v, tv]) => {
        if (cancelled) return
        setVersion(v)
        setTauriVersion(tv)
      })
      .catch((e) => {
        if (!cancelled) setError(String(e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const groups = useMemo(() => {
    const list = manifest.groups as Group[]
    return [...list].sort((a, b) => {
      const d = TIER_ORDER.indexOf(a.tier) - TIER_ORDER.indexOf(b.tier)
      return d !== 0 ? d : b.count - a.count
    })
  }, [])

  const totals = manifest.totals as Record<string, number>
  const byTier = (tier: Tier) => groups.filter((g) => g.tier === tier)
  const needsAttention = [...byTier('copyleft'), ...byTier('unknown')]

  /**
   * 导出 NOTICE.txt。
   *
   * 为什么**必须**能导出：Apache-2.0 要求随分发件附带 NOTICE，
   * MPL-2.0 要求附带其源码与许可。只在页面上「展示」不满足随分发件提供的义务。
   */
  const exportNotice = useCallback(async () => {
    setNotice('idle')
    let dest: string | null
    try {
      dest = await dialogSave({
        title: t('about.exportNotice'),
        defaultPath: 'NOTICE.txt',
        filters: [{ name: 'Text', extensions: ['txt'] }],
      })
    } catch (e) {
      setError(String(e))
      setNotice('fail')
      return
    }
    if (!dest) {
      setNotice('cancelled')
      return
    }
    try {
      await writeTextFile(dest, buildNotice(groups, totals))
      setNotice('ok')
    } catch (e) {
      setError(String(e))
      setNotice('fail')
    }
  }, [groups, totals, t])

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4 pb-10">
      <header>
        <h2 className="text-[length:var(--app-text-lg)] font-semibold">{t('about.title')}</h2>
        <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          {t('about.subtitle')}
        </p>
      </header>

      {/* ---- 版本 ---- */}
      <section className="modern-card p-4">
        <h3 className="text-[length:var(--app-text-md)] font-semibold">{t('about.versionTitle')}</h3>
        {error ? (
          <div className="mt-2 space-y-2">
            <p className="text-[length:var(--app-text-xs)] text-[var(--danger)]">
              {t('about.versionError')}: {error}
            </p>
            <Button size="xs" icon="refresh" onClick={() => setError(null)}>
              {t('common.close')}
            </Button>
          </div>
        ) : loading ? (
          // 加载态不能只用一个「—」占位：那是「读到了但是空」的视觉，
          // 与「还没读到」长得一样，用户分不出是自己点错了还是还没好。
          <p className="mt-2 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
            {t('about.versionLoading')}
          </p>
        ) : (
          <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-[length:var(--app-text-xs)]">
            <dt className="text-[var(--text-secondary)]">{t('about.appVersion')}</dt>
            <dd className="tnum">{version || '—'}</dd>
            <dt className="text-[var(--text-secondary)]">{t('about.runtime')}</dt>
            {/* 单一插值而不是 `{t('about.tauri')} {tauriVersion}`：
                两段文本节点会被读屏断成两个片段，测试也只能用正则绕。 */}
            <dd className="tnum">
              {tauriVersion ? t('about.runtimeValue', { version: tauriVersion }) : '—'}
            </dd>
          </dl>
        )}
      </section>

      {/* ---- 本项目许可 ---- */}
      <section className="modern-card p-4">
        <h3 className="text-[length:var(--app-text-md)] font-semibold">{t('about.licenseTitle')}</h3>
        <p className="mt-2 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          {t('about.licenseSelf')}
        </p>
        <p className="mt-1.5 text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">
          {t('about.licenseScopeHint')}
        </p>
      </section>

      {/* ---- 第三方 ---- */}
      <section className="modern-card p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-[length:var(--app-text-md)] font-semibold">
            {t('about.thirdPartyTitle')}
          </h3>
          <Button size="sm" icon="download" onClick={() => void exportNotice()}>
            {t('about.exportNotice')}
          </Button>
        </div>
        <p className="mt-1 text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">
          {t('about.thirdPartySource', { count: totals.all })}
        </p>
        {notice === 'ok' && (
          <p className="mt-2 text-[length:var(--app-text-xs)] text-[var(--success)]">
            {t('about.noticeExported')}
          </p>
        )}
        {notice === 'fail' && (
          <p className="mt-2 text-[length:var(--app-text-xs)] text-[var(--danger)]">
            {t('about.noticeFailed')}
          </p>
        )}

        <dl className="mt-3 grid grid-cols-2 gap-2 sm:grid-cols-4">
          {TIER_ORDER.map((tier) => (
            <div key={tier} className="rounded-md bg-[var(--bg-card)] px-2.5 py-2">
              <dt className="text-[length:var(--app-text-2xs)] text-[var(--text-secondary)]">
                {t(`about.tier.${tier}`)}
              </dt>
              <dd className="tnum text-[length:var(--app-text-md)] font-semibold">
                {totals[`tier_${tier}`] ?? 0}
              </dd>
            </div>
          ))}
        </dl>

        {/* 非宽松项默认展开：这些才是需要人做决定的东西 */}
        {needsAttention.length > 0 && (
          <div className="mt-4 space-y-3">
            <h4 className="text-[length:var(--app-text-sm)] font-semibold">
              {t('about.needsAttention')}
            </h4>
            {ALLOWLIST.map((n) => (
              <div key={n.name} className="rounded-md border border-[var(--border)] p-3">
                <p className="text-[length:var(--app-text-xs)] font-medium">{n.name}</p>
                <p className="mt-1 text-[length:var(--app-text-2xs)] text-[var(--text-secondary)]">
                  {n.reason}
                </p>
                <p className="mt-1 text-[length:var(--app-text-2xs)] text-[var(--warning)]">
                  {n.obligation}
                </p>
              </div>
            ))}
          </div>
        )}

        <div className="mt-4">
          <Button
            size="xs"
            variant="ghost"
            // 展开/收起用同一个图标：项目图标集里没有 chevron-up，
            // 靠文案区分状态比为此加一个图标更省，也更不容易看错
            icon="arrow-down"
            onClick={() => setExpanded((v) => !v)}
          >
            {expanded ? t('about.hideList') : t('about.showList', { count: totals.all })}
          </Button>
          {expanded && (
            <ul className="mt-2 flex flex-col gap-1.5">
              {groups.map((g) => (
                <li key={`${g.ecosystem}:${g.license}`} className="flex items-start gap-2">
                  <span
                    className={`mt-0.5 shrink-0 rounded px-1.5 py-0.5 text-[length:var(--app-text-micro)] ${
                      g.tier === 'permissive'
                        ? 'bg-[var(--bg-card)] text-[var(--text-secondary)]'
                        : g.tier === 'copyleft'
                          ? 'bg-[var(--warning)]/10 text-[var(--warning)]'
                          : 'bg-[var(--bg-hover)] text-[var(--text-primary)]'
                    }`}
                  >
                    {t(`about.tier.${g.tier}`)}
                  </span>
                  <span className="min-w-0 flex-1 text-[length:var(--app-text-2xs)] break-words">
                    <span className="font-mono">{g.license}</span>
                    {g.nonSpdx && (
                      <span className="ml-1 text-[var(--warning)]">
                        ({t('about.nonSpdx')})
                      </span>
                    )}
                    <span className="text-[var(--text-muted)]">
                      {' · '}
                      {g.ecosystem === 'cargo' ? 'Rust' : 'npm'} · {g.count}
                    </span>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </section>

      {/* ---- 致谢 ---- */}
      <section className="modern-card p-4">
        <h3 className="text-[length:var(--app-text-md)] font-semibold">{t('about.creditsTitle')}</h3>
        <ul className="mt-2 flex flex-col gap-1.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          {(['tauri', 'react', 'treeSitter', 'harmony'] as const).map((k) => (
            <li key={k}>{t(`about.credits.${k}`)}</li>
          ))}
        </ul>
      </section>
    </div>
  )
}

/**
 * 组装 NOTICE.txt 正文。抽成纯函数是为了让导出内容可被测试断言。
 *
 * **正文刻意不 i18n**：NOTICE 是要随分发件给出去的法务文本，
 * 单一语言（英文）比「跟着界面语言变」更合适——分发到中文区也要附同一份。
 * 界面上展示的那几条说明走 i18n，导出的文件走这里的固定文案，两者职责不同。
 */
export function buildNotice(groups: Group[], totals: Record<string, number>): string {
  const lines: string[] = []
  lines.push('DevEco Switch — Third-Party Notices')
  lines.push('='.repeat(64))
  lines.push('')
  lines.push(`Total third-party components: ${totals.all}`)
  lines.push(
    `  permissive (no obligation): ${totals.tier_permissive ?? 0}`,
  )
  lines.push(`  notice (license text required): ${totals.tier_notice ?? 0}`)
  lines.push(`  copyleft: ${totals.tier_copyleft ?? 0}`)
  lines.push(`  unknown (no SPDX identifier): ${totals.tier_unknown ?? 0}`)
  lines.push('')
  lines.push(
    'This file was generated from Cargo.lock (via `cargo metadata`) and',
    'package-lock.json + node_modules/*/package.json.',
    'The application itself is MIT licensed; that license does NOT cover the',
    'third-party components listed below, each of which keeps its own license.',
  )
  lines.push('')
  for (const tier of TIER_ORDER) {
    const list = groups.filter((g) => g.tier === tier)
    if (list.length === 0) continue
    lines.push('-'.repeat(64))
    lines.push(`[${tier.toUpperCase()}] ${list.length} license group(s)`)
    lines.push('-'.repeat(64))
    for (const g of list) {
      lines.push('')
      lines.push(`  License: ${g.license}${g.nonSpdx ? '  (no SPDX identifier)' : ''}`)
      lines.push(`  Ecosystem: ${g.ecosystem}   Components: ${g.count}`)
      for (const n of g.names) lines.push(`    - ${n}`)
    }
    lines.push('')
  }
  lines.push('-'.repeat(64))
  lines.push('Notes')
  lines.push('-'.repeat(64))
  for (const n of ALLOWLIST) {
    lines.push('')
    lines.push(`* ${n.name} (${n.ecosystem})`)
    lines.push(`  Why: ${n.reason}`)
    lines.push(`  Obligation: ${n.obligation}`)
  }
  lines.push('')
  return lines.join('\n')
}
