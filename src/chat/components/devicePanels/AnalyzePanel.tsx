/**
 * 工程能力分析面板：构建错误智能分析 + Kit/权限/依赖/模块盘点 + ohpm 依赖管理。
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useProjectStore } from '../../../stores/projectStore'
import {
  getHarmonyRoot,
  setHarmonyProjectPath,
  rescanWorkspaceModules,
  type HarmonyRootInfo,
} from '../../../api/project'
import {
  analyzeHarmonyProject,
  analyzeHarmonyImpact,
  analyzeGenericProject,
  checkOhpmDeps,
  runOhpmInstall,
  type ProjectCapability,
  type HarmonyImpactAnalysis,
  type AnalyzedBuildError,
  type OhpmDepCheck,
} from '../../../api/harmonyAnalyze'
import Icon, { type IconName } from '../../../icons/Icon'

/* ============ 工程能力分析面板：构建错误智能分析 + Kit/权限/依赖/模块盘点 + ohpm 依赖管理 ============ */
export function AnalyzePanel({
  projectPath,
  projectId,
  projectName,
  onRunBuild,
  onFixErrors,
  onAutoFix,
  refreshTick,
  moduleScanTick,
  agentBusy,
  onHarmonyRootChanged,
  root,
}: {
  projectPath: string
  projectId: string
  projectName: string
  onRunBuild: () => void
  onFixErrors: (errors: AnalyzedBuildError[]) => void
  onAutoFix: (errors: AnalyzedBuildError[]) => void
  refreshTick: number
  moduleScanTick: number
  agentBusy: boolean
  onHarmonyRootChanged: () => void
  /** 会话工作目录（worktree 模式为 worktree 路径，本地模式为 undefined） */
  root?: string
}) {
  const { t } = useTranslation()
  const [data, setData] = useState<ProjectCapability | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // 非鸿蒙工程（混合工作区其它子工程 / 纯其它语言工程）的通用概览文本
  const [genericOverview, setGenericOverview] = useState<string | null>(null)
  const [sub, setSub] = useState<'errors' | 'cap' | 'impact' | 'deps'>('errors')
  const [fixing, setFixing] = useState(false)
  // 会话"鸿蒙主工程"：混合工作区中实际进行鸿蒙开发的子工程（未配置时自动兜底）
  const [rootInfo, setRootInfo] = useState<HarmonyRootInfo | null>(null)
  const [rootLoading, setRootLoading] = useState(false)
  const rescannedRef = useRef(false)
  // ohpm 依赖版本核对 + 安装
  const [depChecks, setDepChecks] = useState<OhpmDepCheck[] | null>(null)
  const [depsLoading, setDepsLoading] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [depsMsg, setDepsMsg] = useState<string | null>(null)
  const [impactInput, setImpactInput] = useState('')
  const [impact, setImpact] = useState<HarmonyImpactAnalysis | null>(null)
  const [impactLoading, setImpactLoading] = useState(false)
  const [impactError, setImpactError] = useState<string | null>(null)
  // 修复闭环反馈：记录 Agent 修复前的错误数，刷新后若清零则显示成功提示
  const [fixedFrom, setFixedFrom] = useState(0)
  // 记录"点击自动修复"瞬间的错误数，作为修复成功判定基准
  const triggerFixSnapshot = useCallback((count: number) => {
    setFixedFrom(count)
  }, [])

  // 解析"鸿蒙主工程"：配置了用配置；未配置且工作区仅一个鸿蒙模块时自动兜底；
  // 无任何鸿蒙候选时尝试触发一次工作区扫描（旧项目可能未扫描过）再查
  useEffect(() => {
    if (!projectId) return
    let cancelled = false
    setRootLoading(true)
    void (async () => {
      try {
        let info = await getHarmonyRoot(projectId, root)
        if (
          !cancelled &&
          !rescannedRef.current &&
          info.candidates.length === 0 &&
          info.configured == null &&
          info.root === projectPath
        ) {
          rescannedRef.current = true
          try {
            await rescanWorkspaceModules(projectId)
            if (!cancelled) info = await getHarmonyRoot(projectId, root)
          } catch {
            // 扫描失败静默，保留原解析结果
          }
        }
        if (!cancelled) setRootInfo(info)
      } catch {
        if (!cancelled) setRootInfo(null)
      } finally {
        if (!cancelled) setRootLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [projectId, projectPath, root, moduleScanTick])

  // 实际分析目录：解析后的鸿蒙主工程根（=项目根本身时行为不变）
  const effPath = rootInfo?.root ?? projectPath

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    setGenericOverview(null)
    setImpact(null)
    setImpactError(null)
    try {
      const cap = await analyzeHarmonyProject(effPath)
      setData(cap)
    } catch {
      // 目标不是鸿蒙工程（混合工作区中的前端/Go/Java 等子工程）：降级为通用工程概览
      setData(null)
      try {
        setGenericOverview(await analyzeGenericProject(effPath))
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    } finally {
      setLoading(false)
    }
  }, [effPath])

  // 切换"鸿蒙主工程"：持久化并触发父级刷新项目引用，随后重新分析
  const handleRootChange = async (value: string) => {
    try {
      await setHarmonyProjectPath(projectId, value === projectPath ? '' : value, root)
      onHarmonyRootChanged()
      setRootInfo(await getHarmonyRoot(projectId, root).catch(() => null))
      setData(null)
      setGenericOverview(null)
      setDepChecks(null)
      void load()
    } catch {
      // 设置失败保留原状态
    }
  }

  const rootLabel = (abs: string) => {
    const base = projectPath.replace(/[\\/]+$/, '')
    return abs.startsWith(base + '/') ? abs.slice(base.length + 1) : abs
  }

  useEffect(() => {
    void load()
  }, [load])

  // Agent 任务结束（refreshTick 变化）后自动重新分析
  useEffect(() => {
    if (refreshTick === 0) return
    void load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshTick])

  // 进入「依赖」子 tab 时加载版本核对
  useEffect(() => {
    if (sub !== 'deps' || depChecks !== null) return
    setDepsLoading(true)
    setDepsMsg(null)
    checkOhpmDeps(effPath)
      .then(setDepChecks)
      .catch((e) => setDepsMsg(e instanceof Error ? e.message : String(e)))
      .finally(() => setDepsLoading(false))
  }, [sub, depChecks, effPath])

  const handleInstall = async () => {
    setInstalling(true)
    setDepsMsg(null)
    try {
      const log = await runOhpmInstall(effPath)
      setDepsMsg(`${t('home.analyzeInstalled')}\n${log}`)
      // 安装后重新核对版本
      const checks = await checkOhpmDeps(effPath)
      setDepChecks(checks)
    } catch (e) {
      setDepsMsg(e instanceof Error ? e.message : String(e))
    } finally {
      setInstalling(false)
    }
  }

  const handleImpact = async () => {
    const changedFiles = impactInput
      .split(/[\n,]/)
      .map((path) => path.trim())
      .filter(Boolean)
    if (changedFiles.length === 0) {
      setImpactError(t('home.analyzeImpactRequired'))
      return
    }
    setImpactLoading(true)
    setImpactError(null)
    try {
      setImpact(await analyzeHarmonyImpact(effPath, changedFiles))
    } catch (e) {
      setImpactError(e instanceof Error ? e.message : String(e))
    } finally {
      setImpactLoading(false)
    }
  }

  const errCount = data?.build_errors.length ?? 0

  // 修复闭环反馈：Agent 修复前记录的错误数 > 0，刷新后错误清零 → 显示修复成功并沉淀工程记忆
  const memorySavedRef = useRef(false)
  const fixedCatsRef = useRef<string>('')
  useEffect(() => {
    if (!data) return
    if (fixedFrom > 0 && errCount === 0 && !memorySavedRef.current) {
      memorySavedRef.current = true
      // 自动淡出成功提示（8s 后清除基准）
      const t = window.setTimeout(() => setFixedFrom(0), 8000)
      // 沉淀一条 pitfall 记忆：记录该工程曾出现并修复构建错误，供后续对话参考
      const cats = fixedCatsRef.current || '其他'
      void useProjectStore.getState().saveMemory({
        category: 'pitfall',
        title: `构建错误自动修复记录（${fixedFrom} 处）`,
        content: `该工程最近一次构建出现 ${fixedFrom} 处错误（分类：${cats}），经 Agent 自动修复后构建已通过。如再次遇到同类问题，可参考本次修复思路：依赖类先 ohpm install；SDK 类核对 compatibleSdkVersion；类型/语法类阅读错误行号上下文修正；签名类需在 DevEco 配置。`,
      }).catch(() => {})
      return () => window.clearTimeout(t)
    }
    if (fixedFrom === 0) memorySavedRef.current = false
  }, [data, fixedFrom, errCount, projectId])

  const errColor = (kind: string) => {
    switch (kind) {
      case 'ERROR':
      case 'ArkTS:ERROR':
        return 'bg-[var(--danger)]/10 text-[var(--danger)]'
      case 'WARNING':
      case 'warning':
        return 'bg-amber-500/10 text-amber-500'
      default:
        return 'bg-[var(--accent-soft)] text-[var(--accent)]'
    }
  }

  const subTabs: { key: typeof sub; label: string; badge?: number; icon: IconName }[] = [
    { key: 'errors', label: t('home.analyzeErrors'), badge: errCount, icon: 'close' },
    { key: 'cap', label: t('home.analyzeCap'), icon: 'spark' },
    { key: 'impact', label: t('home.analyzeImpact'), icon: 'git-branch' },
    { key: 'deps', label: t('home.analyzeDeps'), icon: 'package' },
  ]

  return (
    <div className="p-3 space-y-2.5">
      {/* 头部：项目名 + 刷新/构建 */}
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <div className="text-[length:var(--app-text-md)] font-medium truncate">{projectName}</div>
          <div className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] truncate font-mono" title={projectPath}>
            {projectPath}
          </div>
        </div>
        <button
          type="button"
          onClick={() => void load()}
          title={t('home.analyzeRefresh')}
          aria-label={t('home.analyzeRefresh')}
          className="p-1.5 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
        >
          <Icon name="refresh" size={13} className={loading ? 'animate-spin' : ''} />
        </button>
        {!genericOverview && (
          <button
            type="button"
            onClick={onRunBuild}
            title={t('home.analyzeRunBuild')}
            className="h-6 px-2 rounded-md text-[length:var(--app-text-2xs)] font-medium bg-[var(--accent-soft)] text-[var(--accent)] hover:opacity-80 transition-opacity flex items-center gap-1"
          >
            <Icon name="bolt" size={10} />
            {t('home.analyzeRunBuild')}
          </button>
        )}
      </div>

      {/* 鸿蒙主工程选择器：混合工作区（项目根非鸿蒙工程）时切换实际分析的子工程 */}
      {rootInfo &&
        (rootInfo.candidates.length > 0 || rootInfo.configured != null || rootInfo.auto) && (
          <div className="flex items-center gap-1.5 rounded-lg modern-card px-2 py-1.5">
            <span className="shrink-0 text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">
              {t('home.analyzeHarmonyRoot')}
            </span>
            <select
              value={rootInfo.root}
              disabled={rootLoading}
              onChange={(e) => void handleRootChange(e.target.value)}
              className="h-6 flex-1 min-w-0 rounded-md border border-[var(--border)] bg-[var(--bg-window)] px-1.5 text-[length:var(--app-text-xs)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
            >
              <option value={projectPath}>{projectName}</option>
              {rootInfo.candidates.map((c) => (
                <option key={c} value={c}>
                  {rootLabel(c)}
                </option>
              ))}
            </select>
            {rootInfo.auto && !rootInfo.configured && (
              <span className="shrink-0 rounded bg-[var(--accent-soft)] px-1 py-0.5 text-[length:var(--app-text-micro)] text-[var(--accent)]">
                {t('home.analyzeRootAuto')}
              </span>
            )}
          </div>
        )}

      {/* 子 tab：分段控件（构建错误 / 能力 / 依赖）；非鸿蒙工程（通用概览）时隐藏 */}
      {!genericOverview && (
        <div className="flex gap-0.5 p-0.5 rounded-lg bg-[var(--bg-hover)]/50 border border-[var(--border)]">
        {subTabs.map((tb) => (
          <button
            key={tb.key}
            type="button"
            onClick={() => setSub(tb.key)}
            title={tb.badge ? `${tb.label} (${tb.badge})` : tb.label}
            className={`flex items-center justify-center gap-1 flex-1 h-6 rounded-[6px] text-[length:var(--app-text-xs)] transition-[color,background-color,border-color,box-shadow] border ${
              sub === tb.key
                ? 'tab-soft font-medium border-[var(--border)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] border-transparent'
            }`}
          >
            <Icon name={tb.icon} size={11} className={sub === tb.key ? '' : 'opacity-60'} />
            <span className="whitespace-nowrap">{tb.label}</span>
            {typeof tb.badge === 'number' && tb.badge > 0 && (
              <span
                className={`px-1 rounded text-[length:var(--app-text-micro)] font-mono leading-[14px] ${
                  tb.key === 'errors' ? 'bg-[var(--danger)]/15 text-[var(--danger)]' : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
                }`}
              >
                {tb.badge}
              </span>
            )}
          </button>
        ))}
        </div>
      )}

      {error && (
        <div className="rounded-lg border border-[var(--danger)]/30 bg-[var(--danger)]/5 px-2.5 py-2 text-[length:var(--app-text-xs)] text-[var(--danger)]">
          {error}
        </div>
      )}

      {loading && !data && !genericOverview ? (
        <div className="space-y-2">
          <div className="shimmer h-10 rounded-lg bg-[var(--bg-hover)]" />
          <div className="shimmer h-24 rounded-lg bg-[var(--bg-hover)]" />
          <div className="shimmer h-16 rounded-lg bg-[var(--bg-hover)]" />
        </div>
      ) : (
        <>
          {/* 通用工程概览：非鸿蒙子工程（Node/Go/Rust/Python/Java/C/C++/Flutter/.NET 等） */}
          {genericOverview && !data && !error && (
            <div className="rounded-lg modern-card p-2.5">
              <pre className="whitespace-pre-wrap font-mono text-[length:var(--app-text-xs)] text-[var(--text-secondary)] leading-relaxed">
                {genericOverview}
              </pre>
            </div>
          )}
          {data && (
          <>
            {/* ============ 子 tab：构建错误 ============ */}
            {sub === 'errors' && (
              <div className="space-y-1.5">
                {agentBusy && (
                  <div className="rounded-lg border border-[var(--accent)]/30 bg-[var(--accent-soft)]/40 px-2.5 py-2 text-[length:var(--app-text-xs)] text-[var(--accent)] flex items-center gap-1.5">
                    <Icon name="refresh" size={11} className="animate-spin" />
                    {t('home.analyzeAgentBusy')}
                  </div>
                )}
                {!agentBusy && fixedFrom > 0 && errCount === 0 && (
                  <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-2 text-[length:var(--app-text-xs)] text-emerald-500 flex items-center gap-1.5">
                    <Icon name="check" size={11} />
                    {t('home.analyzeFixSuccess', { n: fixedFrom })}
                  </div>
                )}
                {errCount === 0 ? (
                  <div className="rounded-lg modern-card px-2.5 py-3 text-center text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
                    {t('home.analyzeNoErrors')}
                  </div>
                ) : (
                  <>
                    {data.build_errors.map((e, i) => (
                      <div key={i} className="rounded-lg modern-card p-2.5 space-y-1">
                        <div className="flex items-center gap-1.5 flex-wrap">
                          <span className={`px-1.5 py-0.5 rounded text-[length:var(--app-text-micro)] font-mono font-medium ${errColor(e.kind)}`}>
                            {e.kind}
                          </span>
                          <span className="px-1.5 py-0.5 rounded text-[length:var(--app-text-micro)] bg-[var(--bg-hover)] text-[var(--text-muted)]">
                            {t(`home.cat_${e.category}`, { defaultValue: e.category })}
                          </span>
                          <span className="px-1.5 py-0.5 rounded text-[length:var(--app-text-micro)] bg-[var(--bg-hover)] text-[var(--text-muted)] font-mono">
                            {e.stage || 'build'}
                          </span>
                          {e.error_code && (
                            <span className="px-1.5 py-0.5 rounded text-[length:var(--app-text-micro)] bg-[var(--accent-soft)] text-[var(--accent)] font-mono">
                              {e.error_code}
                            </span>
                          )}
                          {e.file && (
                            <span className="font-mono text-[length:var(--app-text-2xs)] text-[var(--text-secondary)] truncate flex-1 min-w-0" title={e.file}>
                              {e.file}
                              {e.line ? `:${e.line}` : ''}
                              {e.column ? `:${e.column}` : ''}
                            </span>
                          )}
                        </div>
                        <div className="text-[length:var(--app-text-xs)] text-[var(--text-secondary)] break-all">{e.message}</div>
                        {e.suggestion && (
                          <div className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] flex gap-1">
                            <span className="shrink-0 text-[var(--accent)]">{t('home.analyzeSuggestion')}</span>
                            <span>{e.suggestion}</span>
                          </div>
                        )}
                      </div>
                    ))}
                    <div className="flex gap-1.5">
                      <button
                        type="button"
                        onClick={() => {
                          const catSet = new Set(data.build_errors.map((e) => e.category))
                          fixedCatsRef.current = [...catSet].join('/')
                          memorySavedRef.current = false
                          triggerFixSnapshot(data.build_errors.length)
                          void onAutoFix(data.build_errors)
                        }}
                        disabled={fixing || agentBusy}
                        title={t('home.analyzeAutoFix')}
                        className="flex-1 h-7 rounded-lg text-[length:var(--app-text-xs)] font-medium bg-[var(--danger)]/10 text-[var(--danger)] hover:bg-[var(--danger)]/20 transition-colors disabled:opacity-50"
                      >
                        {t('home.analyzeAutoFix')}
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setFixing(true)
                          try {
                            onFixErrors(data.build_errors)
                          } finally {
                            setFixing(false)
                          }
                        }}
                        disabled={fixing}
                        title={t('home.analyzeFixByAgent')}
                        className="h-7 px-2.5 rounded-lg text-[length:var(--app-text-xs)] text-[var(--text-muted)] border border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors disabled:opacity-50"
                      >
                        {t('home.analyzeFixByAgent')}
                      </button>
                    </div>
                  </>
                )}
              </div>
            )}

            {/* ============ 子 tab：工程能力 ============ */}
            {sub === 'cap' && (
              <div className="space-y-2.5">
                {/* 工程摘要：网格统计卡 */}
                <div className="rounded-lg modern-card p-2">
                  <div className="grid grid-cols-2 gap-1.5">
                    {[
                      { label: t('home.analyzeBundle'), value: data.project.bundle_name ?? '--', mono: true, full: true },
                      { label: t('home.analyzeSdk'), value: data.project.sdk_version ?? (data.project.api_version != null ? `API ${data.project.api_version}` : '--'), mono: true, full: false },
                      { label: t('home.analyzeApi'), value: data.project.api_version != null ? `API ${data.project.api_version}` : '--', mono: true, full: false },
                      { label: t('home.analyzeModules'), value: `${data.modules.length}`, mono: true, full: false },
                      { label: t('home.analyzeKits'), value: `${data.kit_usage.length}`, mono: true, full: false },
                      { label: t('home.analyzePermissions'), value: `${data.permissions.length}`, mono: true, full: false },
                    ].map((it) => (
                      <div
                        key={it.label}
                        className={`rounded-md bg-[var(--bg-hover)]/50 border border-[var(--border)]/60 px-2 py-1.5 min-w-0 ${it.full ? 'col-span-2' : ''}`}
                      >
                        <div className="text-[length:var(--app-text-micro)] text-[var(--text-muted)] leading-none mb-1">{it.label}</div>
                        <div className={`text-[length:var(--app-text-xs)] text-[var(--text-secondary)] font-medium truncate ${it.mono ? 'font-mono' : ''}`} title={typeof it.value === 'string' ? it.value : undefined}>
                          {it.value}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>

                {/* 产品构建矩阵：保留配置来源中的 SDK、runtimeOS、签名和模块集合 */}
                {data.semantic_model.products.length > 0 && (
                  <div className="rounded-lg modern-card p-2.5 space-y-2">
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)]">{t('home.analyzeProducts')}</span>
                      <span className="text-[length:var(--app-text-micro)] font-mono text-[var(--text-muted)]">schema v{data.semantic_model.schema_version}</span>
                    </div>
                    {data.semantic_model.products.map((product) => (
                      <div key={product.name} className="rounded-md border border-[var(--border)]/50 bg-[var(--bg-hover)]/40 px-2 py-1.5 space-y-1">
                        <div className="flex items-center gap-1.5">
                          <span className="font-mono text-[length:var(--app-text-xs)] text-[var(--text-secondary)] flex-1 truncate">{product.name}</span>
                          {product.runtime_os && <span className="text-[length:var(--app-text-micro)] text-[var(--text-muted)]">{product.runtime_os}</span>}
                          {product.signing_config && <span className="text-[length:var(--app-text-micro)] text-emerald-500">{t('home.analyzeSigned')}</span>}
                        </div>
                        <div className="font-mono text-[length:var(--app-text-micro)] text-[var(--text-muted)]">
                          compile {product.compile_sdk_version ?? '--'} · compatible {product.compatible_sdk_version ?? '--'} · target {product.target_sdk_version ?? '--'}
                        </div>
                        <div className="flex flex-wrap gap-1">
                          {product.modules.map((module) => (
                            <span key={module} className="rounded bg-[var(--accent-soft)] px-1 py-0.5 font-mono text-[length:var(--app-text-micro)] text-[var(--accent)]">{module}</span>
                          ))}
                        </div>
                      </div>
                    ))}
                    {data.semantic_model.product_differences.map((difference) => (
                      <div key={`${difference.baseline}-${difference.product}`} className="text-[length:var(--app-text-micro)] text-[var(--text-muted)]">
                        {difference.product} ↔ {difference.baseline}: {difference.fields.join(', ')}
                      </div>
                    ))}
                  </div>
                )}

                {/* 模块列表 */}
                {data.modules.length > 0 && (
                  <div className="rounded-lg modern-card p-2.5 space-y-2">
                    <div className="flex items-center gap-1.5">
                      <span className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)]">{t('home.analyzeModuleList')}</span>
                      <span className="px-1.5 py-0.5 rounded-full text-[length:var(--app-text-micro)] font-medium bg-[var(--accent)]/10 text-[var(--accent)]">
                        {data.modules.length}
                      </span>
                    </div>
                    {data.modules.map((m) => (
                      <div key={m.rel_path} className="space-y-0.5 rounded-md bg-[var(--bg-hover)]/40 border border-[var(--border)]/50 px-2 py-1.5">
                        <div className="flex items-center gap-1.5">
                          <Icon name="package" size={10} className="text-[var(--accent)]/70 shrink-0" />
                          <span className="font-mono text-[length:var(--app-text-xs)] text-[var(--text-secondary)] truncate flex-1" title={m.rel_path}>
                            {m.rel_path}
                          </span>
                          <span className="px-1 py-0.5 rounded text-[length:var(--app-text-micro)] bg-[var(--bg-hover)] text-[var(--text-muted)] font-mono">
                            {m.kind || 'module'}
                          </span>
                        </div>
                        {m.device_types.length > 0 && (
                          <div className="pl-4 flex flex-wrap gap-1">
                            {m.device_types.map((dt) => (
                              <span key={dt} className="px-1 py-0.5 rounded text-[length:var(--app-text-micro)] bg-[var(--accent-soft)] text-[var(--accent)]">
                                {dt}
                              </span>
                            ))}
                          </div>
                        )}
                        {m.kits.length > 0 && (
                          <div className="pl-4 flex flex-wrap gap-1">
                            {m.kits.map((k) => (
                              <span key={k} className="font-mono text-[length:var(--app-text-micro)] px-1 py-0.5 rounded bg-[var(--bg-hover)] text-[var(--text-muted)]">
                                {k}
                              </span>
                            ))}
                          </div>
                        )}
                        {(() => {
                          const modelModule = data.semantic_model.modules.find((module) => module.rel_path === m.rel_path)
                          if (!modelModule) return null
                          const abilityNames = [...modelModule.abilities, ...modelModule.extension_abilities].map((ability) => ability.name)
                          return (
                            <div className="pl-4 text-[length:var(--app-text-micro)] text-[var(--text-muted)] space-y-0.5">
                              <div className="font-mono">{modelModule.artifact_kind.toUpperCase()} · {modelModule.src_path}</div>
                              {abilityNames.length > 0 && <div>{t('home.analyzeAbilities')}: {abilityNames.join(', ')}</div>}
                            </div>
                          )
                        })()}
                      </div>
                    ))}
                  </div>
                )}

                {/* Kit 使用 Top：带占比进度条 */}
                {data.kit_usage.length > 0 && (() => {
                  const maxCount = data.kit_usage[0]?.count ?? 1
                  return (
                    <div className="rounded-lg modern-card p-2.5">
                      <div className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)] mb-1.5">{t('home.analyzeKitUsage')}</div>
                      <div className="space-y-1.5">
                        {data.kit_usage.slice(0, 12).map((k) => (
                          <div key={k.kit}>
                            <div className="flex items-center gap-2">
                              <span className="font-mono text-[length:var(--app-text-2xs)] text-[var(--text-secondary)] flex-1 min-w-0 truncate" title={k.kit}>
                                {k.kit}
                              </span>
                              <span className="text-[length:var(--app-text-micro)] font-mono text-[var(--text-muted)] w-8 text-right">{k.count}</span>
                            </div>
                            <div className="h-[3px] mt-0.5 rounded-full bg-[var(--bg-hover)] overflow-hidden">
                              <div
                                className="h-full rounded-full bg-gradient-to-r from-[var(--accent)]/70 to-[var(--accent)]/30 transition-[width]"
                                style={{ width: `${Math.max(6, (k.count / maxCount) * 100)}%` }}
                              />
                            </div>
                          </div>
                        ))}
                      </div>
                    </div>
                  )
                })()}

                {/* 权限 */}
                {data.permissions.length > 0 && (
                  <div className="rounded-lg modern-card p-2.5">
                    <div className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)] mb-1.5">{t('home.analyzePermissions')}</div>
                    <div className="space-y-1">
                      {data.permissions.map((p) => (
                        <div key={p.name} className="flex items-center gap-1.5">
                          <span className="font-mono text-[length:var(--app-text-2xs)] text-[var(--text-secondary)] flex-1 min-w-0 truncate" title={p.name}>
                            {p.name}
                          </span>
                          {p.reason && (
                            <span className="text-[length:var(--app-text-micro)] text-[var(--text-muted)] truncate max-w-[110px]" title={p.reason}>
                              {p.reason}
                            </span>
                          )}
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                {/* 可追溯来源：清单状态和关系图证据 */}
                <div className="rounded-lg modern-card p-2.5 space-y-1.5">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)]">{t('home.analyzeSources')}</span>
                    <span className="text-[length:var(--app-text-micro)] text-[var(--text-muted)]">
                      {data.semantic_model.graph.edges.length} {t('home.analyzeEdges')}
                    </span>
                  </div>
                  {data.semantic_model.manifests.map((manifest) => (
                    <div key={`${manifest.kind}-${manifest.path}`} className="flex items-center gap-1.5 text-[length:var(--app-text-micro)]">
                      <span className={`h-1.5 w-1.5 rounded-full shrink-0 ${manifest.status === 'parsed' ? 'bg-emerald-500' : 'bg-[var(--danger)]'}`} />
                      <span className="font-mono text-[var(--text-secondary)] truncate flex-1" title={manifest.path}>{manifest.path}</span>
                      <span className="text-[var(--text-muted)]">{manifest.kind}</span>
                    </div>
                  ))}
                  {data.semantic_model.manifests.some((manifest) => manifest.status === 'invalid') && (
                    <div className="rounded bg-[var(--danger)]/5 px-1.5 py-1 text-[length:var(--app-text-micro)] text-[var(--danger)]">
                      {data.semantic_model.manifests.filter((manifest) => manifest.status === 'invalid').map((manifest) => `${manifest.path}: ${manifest.error ?? 'invalid'}`).join('\n')}
                    </div>
                  )}
                </div>
              </div>
            )}

            {/* ============ 子 tab：变更影响 ============ */}
            {sub === 'impact' && (
              <div className="space-y-2">
                <div className="rounded-lg modern-card p-2.5 space-y-2">
                  <div className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">{t('home.analyzeImpactHint')}</div>
                  <textarea
                    value={impactInput}
                    onChange={(event) => setImpactInput(event.target.value)}
                    placeholder={t('home.analyzeImpactPlaceholder')}
                    rows={4}
                    className="w-full resize-y rounded-md border border-[var(--border)] bg-[var(--bg-window)] px-2 py-1.5 font-mono text-[length:var(--app-text-2xs)] text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
                  />
                  <button
                    type="button"
                    onClick={() => void handleImpact()}
                    disabled={impactLoading}
                    className="h-7 w-full rounded-md bg-[var(--accent-soft)] text-[length:var(--app-text-xs)] font-medium text-[var(--accent)] disabled:opacity-50"
                  >
                    {impactLoading ? t('home.analyzeImpactRunning') : t('home.analyzeImpactRun')}
                  </button>
                  {impactError && <div className="text-[length:var(--app-text-2xs)] text-[var(--danger)]">{impactError}</div>}
                </div>
                {impact && (
                  <>
                    <div className="rounded-lg modern-card p-2.5 space-y-1.5">
                      <div className="flex items-center gap-1.5">
                        <span className={`rounded px-1.5 py-0.5 text-[length:var(--app-text-micro)] font-medium ${impact.mode === 'full' ? 'bg-amber-500/10 text-amber-500' : 'bg-emerald-500/10 text-emerald-500'}`}>
                          {impact.mode === 'full' ? t('home.analyzeImpactFull') : t('home.analyzeImpactIncremental')}
                        </span>
                        <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">{impact.affected_modules.length} {t('home.analyzeModules')}</span>
                      </div>
                      <div className="flex flex-wrap gap-1">
                        {impact.verification.checks.map((check) => <span key={check} className="rounded bg-[var(--accent-soft)] px-1 py-0.5 font-mono text-[length:var(--app-text-micro)] text-[var(--accent)]">{check}</span>)}
                        {impact.verification.products.map((product) => <span key={product} className="rounded bg-[var(--bg-hover)] px-1 py-0.5 font-mono text-[length:var(--app-text-micro)] text-[var(--text-muted)]">product:{product}</span>)}
                      </div>
                    </div>
                    <div className="rounded-lg modern-card p-2.5 space-y-1.5">
                      <div className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)]">{t('home.analyzeImpactTrace')}</div>
                      {impact.traces.map((trace) => (
                        <div key={`${trace.module}-${trace.kind}`} className="rounded-md border border-[var(--border)]/50 bg-[var(--bg-hover)]/40 px-2 py-1.5">
                          <div className="flex items-center gap-1.5">
                            <span className="font-mono text-[length:var(--app-text-2xs)] text-[var(--text-secondary)] flex-1 truncate">{trace.module}</span>
                            <span className="text-[length:var(--app-text-micro)] text-[var(--accent)]">{trace.kind}</span>
                          </div>
                          <div className="font-mono text-[length:var(--app-text-micro)] text-[var(--text-muted)] break-all">
                            {trace.depends_on ? `← ${trace.depends_on} · ` : ''}{trace.source}
                          </div>
                        </div>
                      ))}
                    </div>
                  </>
                )}
              </div>
            )}

            {/* ============ 子 tab：ohpm 依赖 ============ */}
            {sub === 'deps' && (
              <div className="space-y-1.5">
                {/* 版本核对 + 安装操作 */}
                <div className="flex items-center gap-1.5">
                  <button
                    type="button"
                    onClick={() => void handleInstall()}
                    disabled={installing}
                    className="h-6 px-2 rounded-md text-[length:var(--app-text-2xs)] font-medium bg-[var(--accent-soft)] text-[var(--accent)] hover:opacity-80 transition-opacity flex items-center gap-1 disabled:opacity-50"
                  >
                    <Icon name="download" size={10} className={installing ? 'animate-spin' : ''} />
                    {installing ? t('home.analyzeInstalling') : t('home.analyzeInstall')}
                  </button>
                  {depsLoading && (
                    <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">{t('home.analyzeCheckingDeps')}</span>
                  )}
                </div>
                {depsMsg && (
                  <div className="rounded-lg modern-card px-2.5 py-2 text-[length:var(--app-text-2xs)] font-mono text-[var(--text-secondary)] whitespace-pre-wrap break-all max-h-40 overflow-y-auto">
                    {depsMsg}
                  </div>
                )}
                {depChecks === null && !depsLoading ? (
                  <div className="rounded-lg modern-card px-2.5 py-3 text-center text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
                    {t('home.analyzeNoDeps')}
                  </div>
                ) : depChecks && depChecks.length === 0 ? (
                  <div className="rounded-lg modern-card px-2.5 py-3 text-center text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
                    {t('home.analyzeNoDeps')}
                  </div>
                ) : (
                  depChecks && (
                    (() => {
                      // 按模块分组：根依赖（module=""）在前
                      const groups = new Map<string, OhpmDepCheck[]>()
                      for (const d of depChecks) {
                        const key = d.module || '(root)'
                        if (!groups.has(key)) groups.set(key, [])
                        groups.get(key)!.push(d)
                      }
                      return [...groups.entries()].map(([mod, deps]) => (
                        <div key={mod} className="rounded-lg modern-card p-2.5">
                          <div className="flex items-center gap-1.5 mb-1">
                            <Icon name="package" size={10} className="text-[var(--accent)] shrink-0" />
                            <span className="font-mono text-[length:var(--app-text-xs)] text-[var(--text-secondary)] truncate flex-1">{mod}</span>
                            <span className="text-[length:var(--app-text-micro)] text-[var(--text-muted)]">{deps.length}</span>
                          </div>
                          <div className="space-y-1">
                            {deps.map((d) => {
                              const missing = !d.installed
                              const outdated = d.installed && d.declared && !d.declared.includes(d.installed)
                              return (
                                <div key={`${d.name}-${d.declared}`} className="text-[length:var(--app-text-2xs)]">
                                  <div className="flex items-center gap-1.5">
                                    <span className="font-mono text-[var(--text-secondary)] flex-1 min-w-0 truncate" title={d.name}>
                                      {d.name}
                                    </span>
                                    <span className="font-mono text-[var(--text-muted)]">{d.declared}</span>
                                    {d.dev && (
                                      <span className="px-1 rounded text-[length:var(--app-text-micro)] bg-[var(--bg-hover)] text-[var(--text-muted)]">
                                        {t('home.analyzeDevDep')}
                                      </span>
                                    )}
                                  </div>
                                  {missing ? (
                                    <div className="pl-3 text-[length:var(--app-text-micro)] text-[var(--danger)]">
                                      {t('home.analyzeNotInstalled')}
                                    </div>
                                  ) : (
                                    outdated && (
                                      <div className="pl-3 text-[length:var(--app-text-micro)] text-amber-500">
                                        {t('home.analyzeInstalledVer')} {d.installed}
                                      </div>
                                    )
                                  )}
                                </div>
                              )
                            })}
                          </div>
                        </div>
                      ))
                    })()
                  )
                )}
              </div>
            )}
          </>
          )}
        </>
      )}
    </div>
  )
}
