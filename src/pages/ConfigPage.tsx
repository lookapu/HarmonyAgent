// @ui-states: loading, failed, retry
/**
 * 设置页：三段式（常规 / 数据 / 高级）
 *
 * ## 为什么是这个结构
 *
 * 此前整页只有一个裸 JSON textarea 直接暴露 `deveco.jsonc`。查过读写链路后
 * 确认：**这个文件不是本应用的设置**——前端没有任何 store 读它，后端只有
 * `import_providers` 在导入 Provider 时读一次 `provider` 段。真正的应用设置
 * 分散在 localStorage（主题/语言/工具链/布局）里。
 *
 * 于是「让用户改 JSON」这个界面既暴露了实现细节，又**改不动任何实际设置**：
 * 用户在这里保存一个主题值，界面上不会有任何变化。
 *
 * 现在的分工：
 * - **常规**：真设置。主题与语言走各自真实的 store，即时生效。
 * - **数据**：真操作。规模统计 / 维护 / 导出 / 清空（原有内容迁移而来）。
 * - **高级**：折叠的原始 JSON 逃生舱，明说它是 DevEco Studio 配置文件。
 *
 * 高级区默认折叠：它是给排查问题的人用的，不该在常规路径上抢注意力。
 */
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { open } from '@tauri-apps/plugin-dialog'
import {
  readConfig,
  writeConfig,
  getConfigPath,
  clearContentData,
  runMaintenance,
  exportBackup,
  getDataScale,
  previewDiagnosticBundle,
  generateDiagnosticBundle,
  type DataScale,
  type DiagnosticPreview,
} from '../api/config'
import { detectSystemLocale } from '../api/desktop'
import { useThemeStore, type Theme } from '../stores/themeStore'
import { useFontScaleStore, type FontScale } from '../stores/fontScaleStore'
import { LANG_STORAGE_KEY } from '../i18n'
import { getItem, setItem } from '../utils/storage'
import { clearFirstRunSeen } from '../utils/firstRun'
import { isTrackingEnabled, setTrackingEnabled, track } from '../utils/telemetry'
import { Button } from '../components/ui/Button'
import { Segmented } from '../components/ui/Segmented'
import { TextArea } from '../components/ui/Field'
import { ConfirmDialog } from '../components/ui/ConfirmDialog'
import { cn } from '../utils/cn'

type LangMode = 'auto' | 'zh' | 'en'

/** 设置分区的外壳：标题 + 说明 + 内容。三个区共用，保证纵向节奏一致。 */
function Section({
  title,
  desc,
  children,
}: {
  title: string
  desc: string
  children: React.ReactNode
}) {
  return (
    <section className="modern-card p-4">
      <h3 className="text-[length:var(--app-text-md)] font-semibold">{title}</h3>
      <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
        {desc}
      </p>
      <div className="mt-3">{children}</div>
    </section>
  )
}

/** 一行设置：左标签右控件。label 与控件纵向居中对齐，控件右对齐。 */
function Row({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <div className="flex items-center justify-between gap-4 py-1.5">
      <div className="min-w-0">
        <div className="text-[length:var(--app-text-sm)]">{label}</div>
        {hint && (
          <div className="mt-0.5 text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">
            {hint}
          </div>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  )
}

export default function ConfigPage() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()

  // ---- 常规：主题 / 语言 / 字号（真设置，即时生效） ----
  const theme = useThemeStore((s) => s.theme)
  const setTheme = useThemeStore((s) => s.setTheme)
  const fontScale = useFontScaleStore((s) => s.scale)
  const setFontScale = useFontScaleStore((s) => s.setScale)

  // 使用统计：前端读 localStorage 作初始值（省一次 IPC），权威开关在后端 settings 表
  const [trackingOn, setTrackingOn] = useState(() => isTrackingEnabled())
  const changeTracking = useCallback(
    async (next: boolean) => {
      try {
        await setTrackingEnabled(next)
        if (next) track('settings_changed', { key: 'telemetry', value: 'on' })
      } catch (e) {
        // 写不进去必须回滚 UI：否则界面显示「已开启」而实际什么都没记录
        setTrackingOn(!next)
        setActionError(`${t('config.clearError')}: ${e}`)
      }
    },
    [t],
  )

  const [langMode, setLangMode] = useState<LangMode>(() => {
    const saved = getItem(LANG_STORAGE_KEY)
    return saved === 'auto' || saved === 'zh' || saved === 'en' ? saved : 'auto'
  })

  const changeLang = useCallback(
    async (mode: LangMode) => {
      setLangMode(mode)
      if (mode === 'auto') {
        setItem(LANG_STORAGE_KEY, 'auto')
        try {
          const info = await detectSystemLocale()
          i18n.changeLanguage(info.is_zh ? 'zh' : 'en')
        } catch {
          // 探测失败退回中文，与 LangToggle 的兜底一致；不弹错——
          // 语言切不了不是用户能修的，弹错只会让人以为设置坏了
          i18n.changeLanguage('zh')
        }
        return
      }
      setItem(LANG_STORAGE_KEY, mode)
      i18n.changeLanguage(mode)
    },
    [i18n],
  )

  // ---- 数据：规模 / 维护 / 导出 / 清空 ----
  const [scale, setScale] = useState<DataScale | null>(null)
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [confirmClear, setConfirmClear] = useState(false)

  const refreshScale = useCallback(async () => {
    try {
      setScale(await getDataScale())
    } catch {
      // 规模拿不到就整块不显示，不给「0」的假象——
      // 「读不到」与「真的是 0」是两件事
      setScale(null)
    }
  }, [])

  useEffect(() => {
    void refreshScale()
  }, [refreshScale])

  const withBusy = async (fn: () => Promise<void>) => {
    setBusy(true)
    setActionError(null)
    setNote(null)
    try {
      await fn()
    } finally {
      setBusy(false)
    }
  }

  const handleRunMaintenance = () =>
    withBusy(async () => {
      try {
        const [logs, runs] = await runMaintenance()
        setNote(t('config.runCleanDone', { logs, runs }))
        await refreshScale()
      } catch (e) {
        setActionError(`${t('config.clearError')}: ${e}`)
      }
    })

  const handleExport = () =>
    withBusy(async () => {
      let dir: string | null
      try {
        dir = await open({ directory: true, multiple: false })
      } catch {
        return // 用户取消目录选择，不是错误
      }
      if (typeof dir !== 'string' || !dir) return
      try {
        const res = await exportBackup(dir)
        const [path, size] = res.split('|')
        setNote(t('config.exportDone', { path, size: (Number(size) / 1024).toFixed(0) }))
      } catch (e) {
        setActionError(`${t('config.clearError')}: ${e}`)
      }
    })

  const handleClearAll = () =>
    withBusy(async () => {
      try {
        const [convs, msgs] = await clearContentData()
        setNote(t('config.clearAllDone', { convs, msgs }))
        await refreshScale()
      } catch (e) {
        setActionError(`${t('config.clearError')}: ${e}`)
      }
    })

  // ---- 诊断包：必须先预览再导出 ----
  // 后端会以 preview_digest 校验「你看到的」与「导出的」是不是同一份内容，
  // 对不上直接拒绝。所以这里不缓存摘要：预览一过期，导出就会失败并提示重看——
  // 那比让用户导出一份自己没检查过的内容要好。
  const [diagPreview, setDiagPreview] = useState<DiagnosticPreview | null>(null)
  const [diagNote, setDiagNote] = useState<string | null>(null)
  const [diagError, setDiagError] = useState<string | null>(null)

  const handleDiagPreview = () =>
    withBusy(async () => {
      setDiagError(null)
      setDiagNote(null)
      try {
        setDiagPreview(await previewDiagnosticBundle())
      } catch (e) {
        setDiagPreview(null)
        setDiagError(`${t('config.clearError')}: ${e}`)
      }
    })

  const handleDiagExport = () =>
    withBusy(async () => {
      if (!diagPreview) return
      let dir: string | null
      try {
        dir = await open({ directory: true, multiple: false })
      } catch {
        return // 用户取消目录选择，不是错误
      }
      if (typeof dir !== 'string' || !dir) return
      setDiagError(null)
      setDiagNote(null)
      try {
        const record = await generateDiagnosticBundle(dir, diagPreview.preview_digest)
        setDiagNote(t('config.diagExported', { path: record.path }))
      } catch (e) {
        // 摘要对不上 = 预览之后数据变了。这是最需要说清楚的一种失败：
        // 用户刚看完内容清单，导出却失败，必须告诉他「重看一次」而不是「操作失败」。
        const message = String(e)
        setDiagError(
          message.includes('已变化')
            ? t('config.diagStale')
            : `${t('config.clearError')}: ${message}`,
        )
        setDiagPreview(null)
      }
    })

  // ---- 高级：deveco.jsonc 原始编辑（默认折叠） ----
  const [configPath, setConfigPath] = useState('')
  const [configText, setConfigText] = useState('')
  const [advancedOpen, setAdvancedOpen] = useState(false)
  /** 存原始错误串，不存拼好的文案。t 每次渲染可能是新函数，
   *  把它放进 effect 依赖会让「文案变了就重读一遍配置」——本末倒置。 */
  const [loadError, setLoadError] = useState<string | null>(null)
  const [jsonError, setJsonError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  /** 自增即触发重载。失败后「重试」按钮改这个值，而不是收起面板——
   *  收起来再展开也能重读，但用户点了「重试」却什么都没发生，那是个假按钮。 */
  const [reloadToken, setReloadToken] = useState(0)

  // 只在展开时才拉取：绝大多数用户不会进这一区，
  // 没必要让他们进页面就先等一次磁盘读 + JSON.parse
  useEffect(() => {
    if (!advancedOpen) return
    let cancelled = false
    void (async () => {
      try {
        const [path, data] = await Promise.all([getConfigPath(), readConfig()])
        if (cancelled) return
        setConfigPath(path)
        setConfigText(JSON.stringify(data, null, 2))
        setLoadError(null)
      } catch (e) {
        if (!cancelled) setLoadError(String(e))
      }
    })()
    return () => {
      cancelled = true
    }
  }, [advancedOpen, reloadToken])

  const handleSaveJson = async () => {
    setJsonError(null)
    let parsed: unknown
    try {
      parsed = JSON.parse(configText)
    } catch (e) {
      setJsonError(`${t('config.jsonInvalid')}: ${e}`)
      return
    }
    try {
      await writeConfig(parsed as Record<string, unknown>)
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch (e) {
      setJsonError(`${t('config.saveError')}: ${e}`)
    }
  }

  const themeOptions: { value: Theme; label: string }[] = [
    { value: 'dark', label: t('config.themeDark') },
    { value: 'light', label: t('config.themeLight') },
    { value: 'auto', label: t('config.themeAuto') },
  ]
  const langOptions: { value: LangMode; label: string }[] = [
    { value: 'zh', label: '中文' },
    { value: 'en', label: 'English' },
    { value: 'auto', label: t('config.langAuto') },
  ]
  const fontScaleOptions: { value: FontScale; label: string; title?: string }[] = [
    { value: 'sm', label: t('config.fontSm'), title: t('config.fontSmHint') },
    { value: 'md', label: t('config.fontMd') },
    { value: 'lg', label: t('config.fontLg'), title: t('config.fontLgHint') },
  ]

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4 pb-10">
      <header>
        <h2 className="text-[length:var(--app-text-lg)] font-semibold">
          {t('config.title')}
        </h2>
        <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
          {t('config.subtitle')}
        </p>
      </header>

      {/* 常规 */}
      <Section title={t('config.generalSection')} desc={t('config.generalDesc')}>
        <div className="divide-y divide-[var(--border)]">
          <Row label={t('config.theme')} hint={t('config.themeHint')}>
            <Segmented
              label={t('config.theme')}
              options={themeOptions}
              value={theme}
              onChange={setTheme}
            />
          </Row>
          <Row label={t('config.lang')} hint={t('config.langHint')}>
            <Segmented
              label={t('config.lang')}
              options={langOptions}
              value={langMode}
              onChange={(m) => void changeLang(m)}
            />
          </Row>
          <Row label={t('config.fontSize')} hint={t('config.fontSizeHint')}>
            <Segmented
              label={t('config.fontSize')}
              options={fontScaleOptions}
              value={fontScale}
              onChange={setFontScale}
            />
          </Row>
          {/* 引导是「看过就不再打扰」的，但必须能再看一次——否则第一次没看懂的用户
              没有任何入口。改标记 + 回工作区，Home 会在首帧判定并弹出。 */}
          <Row label={t('config.onboarding')} hint={t('config.onboardingHint')}>
            <Button
              icon="spark"
              onClick={() => {
                clearFirstRunSeen()
                navigate('/')
              }}
            >
              {t('config.onboardingReplay')}
            </Button>
          </Row>
          {/* 使用统计是唯一需要用户「授权」才生效的设置：默认关闭，关掉还会清空数据。
              所以用 Segmented 而不是普通开关——「关 = 清空已收集数据」是破坏性操作，
              不该藏在一个无感的 toggle 里。 */}
          <Row label={t('config.telemetry')} hint={t('config.telemetryHint')}>
            <div className="flex items-center gap-2">
              <Segmented
                label={t('config.telemetry')}
                options={[
                  { value: 'off', label: t('common.off') },
                  { value: 'on', label: t('config.telemetryEnable') },
                ]}
                value={trackingOn ? 'on' : 'off'}
                onChange={(v) => {
                  const next = v === 'on'
                  setTrackingOn(next)
                  void changeTracking(next)
                }}
              />
              {trackingOn && (
                <Button size="sm" onClick={() => navigate('/usage')}>
                  {t('nav.usage')}
                </Button>
              )}
            </div>
          </Row>
        </div>
      </Section>

      {/* 数据 */}
      <Section title={t('config.dataSection')} desc={t('config.dataDesc')}>
        {scale && (
          <div className="mb-3 flex flex-wrap gap-2">
            {(
              [
                [t('config.scaleConv'), scale.conversations],
                [t('config.scaleMsg'), scale.messages],
                [t('config.scaleLog'), scale.request_logs],
                [t('config.scaleRun'), scale.task_runs],
                [t('config.scaleMem'), scale.project_memories],
              ] as const
            ).map(([label, value]) => (
              <div
                key={label}
                className="flex items-center gap-2 rounded-md border border-[var(--border)] bg-[var(--bg-primary)] px-2.5 py-1"
              >
                <span className="text-[length:var(--app-text-2xs)] text-[var(--text-secondary)]">
                  {label}
                </span>
                <span className="tnum text-[length:var(--app-text-sm)] font-semibold">
                  {value}
                </span>
              </div>
            ))}
          </div>
        )}

        {note && (
          <p className="mb-2 text-[length:var(--app-text-xs)] text-[var(--success)]">
            {note}
          </p>
        )}
        {actionError && (
          <p className="mb-2 text-[length:var(--app-text-xs)] text-[var(--danger)]">
            {actionError}
          </p>
        )}

        <div className="flex flex-wrap gap-2">
          <Button onClick={handleRunMaintenance} loading={busy} icon="refresh">
            {t('config.runClean')}
          </Button>
          <Button onClick={handleExport} loading={busy} icon="download">
            {t('config.exportBackup')}
          </Button>
          <Button
            variant="danger"
            onClick={() => setConfirmClear(true)}
            disabled={busy}
            icon="delete"
          >
            {t('config.clearAll')}
          </Button>
        </div>

        {/* 诊断包单独成块而不是并进上面那排按钮：
            它和「导出备份」的方向正好相反（一个含你的数据，一个不含），
            挤在同一排按钮里会让人以为点错了。隐私边界必须写在按钮旁边。 */}
        <div className="mt-4 rounded-[length:var(--app-radius-md)] border border-[var(--border)] p-3">
          <h4 className="text-[length:var(--app-text-sm)] font-semibold">
            {t('config.diagSection')}
          </h4>
          <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
            {t('config.diagDesc')}
          </p>
          <p className="mt-1.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
            {t('config.diagIncluded')}
          </p>
          <p className="mt-1 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
            {t('config.diagExcluded')}
          </p>

          <div className="mt-2.5 flex flex-wrap gap-2">
            <Button onClick={handleDiagPreview} loading={busy} icon="search">
              {t('config.diagPreview')}
            </Button>
            <Button
              onClick={handleDiagExport}
              loading={busy}
              disabled={!diagPreview}
              icon="download"
            >
              {t('config.diagExport')}
            </Button>
          </div>
          {!diagPreview && (
            <p className="mt-2 text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
              {t('config.diagNeedPreview')}
            </p>
          )}

          {diagError && (
            <p className="mt-2 text-[length:var(--app-text-xs)] text-[var(--danger)]">
              {diagError}
            </p>
          )}
          {diagNote && (
            <p className="mt-2 break-all text-[length:var(--app-text-xs)] text-[var(--success)]">
              {diagNote}
            </p>
          )}

          {diagPreview && (
            <div className="mt-2.5 rounded-[length:var(--app-radius-sm)] bg-[var(--bg-primary)] p-2.5">
              <div className="flex flex-wrap items-baseline justify-between gap-2">
                <span className="text-[length:var(--app-text-xs)] font-medium">
                  {t('config.diagFiles', { count: diagPreview.entries.length })}
                </span>
                <span className="tnum text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
                  {t('config.diagTotal', {
                    kb: (diagPreview.total_bytes / 1024).toFixed(1),
                  })}
                </span>
              </div>
              <ul className="mt-1.5 space-y-0.5">
                {diagPreview.entries.map((entry) => (
                  <li
                    key={entry.path}
                    className="flex items-baseline justify-between gap-2 text-[length:var(--app-text-xs)]"
                  >
                    <span className="truncate text-[var(--text-secondary)]">{entry.path}</span>
                    <span className="tnum shrink-0 text-[var(--text-muted)]">
                      {(entry.bytes / 1024).toFixed(1)} KB
                    </span>
                  </li>
                ))}
              </ul>
              {/* 条数为 0 的来源也要列出来：用户看到「近期没有错误」和
                  「这个来源没接上」是两回事，不能都显示成空。 */}
              <ul className="mt-2 space-y-0.5 border-t border-[var(--border)] pt-2">
                {diagPreview.source_counts.map((count) => (
                  <li
                    key={count.source}
                    className="flex items-baseline justify-between gap-2 text-[length:var(--app-text-xs)]"
                  >
                    <span className="truncate text-[var(--text-secondary)]">{count.source}</span>
                    <span className="tnum shrink-0 text-[var(--text-muted)]">
                      {count.included === 0
                        ? t('config.diagEmpty')
                        : `${count.included} / ${count.available}`}
                    </span>
                  </li>
                ))}
              </ul>
              {diagPreview.truncated.length > 0 && (
                <p className="mt-1.5 text-[length:var(--app-text-xs)] text-[var(--warning)]">
                  {t('config.diagTruncated')}
                </p>
              )}
            </div>
          )}
        </div>
      </Section>

      {/* 高级 */}
      <section className="modern-card overflow-hidden">
        <button
          type="button"
          aria-expanded={advancedOpen}
          aria-controls="config-advanced-panel"
          onClick={() => setAdvancedOpen((v) => !v)}
          className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left hover:bg-[var(--bg-hover)]"
        >
          <div className="min-w-0">
            <h3 className="text-[length:var(--app-text-md)] font-semibold">
              {t('config.advancedSection')}
            </h3>
            <p className="mt-0.5 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
              {t('config.advancedDesc')}
            </p>
          </div>
          <span
            aria-hidden="true"
            className={cn(
              'shrink-0 text-[var(--text-muted)] transition-transform',
              advancedOpen && 'rotate-90',
            )}
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
              <path
                d="M6 3.5 10.5 8 6 12.5"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </span>
        </button>

        {advancedOpen && (
          <div id="config-advanced-panel" className="border-t border-[var(--border)] p-4">
            {loadError ? (
              <div className="space-y-2">
                <p className="text-[length:var(--app-text-xs)] text-[var(--danger)]">
                  {t('config.loadError')}: {loadError}
                </p>
                <Button size="xs" icon="refresh" onClick={() => setReloadToken((n) => n + 1)}>
                  {t('common.retry')}
                </Button>
              </div>
            ) : (
              <>
                <p className="mb-2 font-mono text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">
                  {configPath}
                </p>
                <TextArea
                  value={configText}
                  onChange={(e) => setConfigText(e.target.value)}
                  spellCheck={false}
                  rows={14}
                  mono
                  label={t('config.advancedLabel')}
                  error={jsonError ?? undefined}
                />
                <div className="mt-3 flex items-center gap-2">
                  <Button variant="primary" onClick={handleSaveJson}>
                    {t('config.save')}
                  </Button>
                  {saved && (
                    <span className="text-[length:var(--app-text-xs)] text-[var(--success)]">
                      {t('config.saved')}
                    </span>
                  )}
                </div>
              </>
            )}
          </div>
        )}
      </section>

      <ConfirmDialog
        open={confirmClear}
        title={t('config.clearAll')}
        body={t('config.clearAllConfirm')}
        confirmLabel={t('config.clearAll')}
        tone="danger"
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => {
          setConfirmClear(false)
          void handleClearAll()
        }}
      />
    </div>
  )
}
