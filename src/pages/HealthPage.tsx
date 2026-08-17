import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { open } from '@tauri-apps/plugin-dialog'
import { open as shellOpen } from '@tauri-apps/plugin-shell'
import Icon from '../icons/Icon'
import { checkAllHealth, checkHarmonyToolchain, type HealthResult, type ToolchainCheck } from '../api/health'
import { getHarmonyEnv, detectHarmonyEnv, saveHarmonyEnv, checkProjectSdkAlignment,
  getHarmonyDocsStatus, updateHarmonyDocs, searchHarmonyDocs, readHarmonyDoc,
  type HarmonyEnv, type ProjectSdkAlignment, type HarmonyDocsStatus, type DocEntry } from '../api/harmonyEnv'
import SdkApiBrowser from '../components/SdkApiBrowser'
import { getNodeRuntime, upgradeNodeRuntime, resetNodeRuntime, type NodeRuntimeInfo } from '../api/nodeRuntime'
import { getGitRuntime, fetchGitLatestVersion, upgradeGitRuntime, resetGitRuntime, type GitRuntimeInfo } from '../api/gitRuntime'
import { getJdkRuntime, fetchJdkReleases, installJdk, setDefaultJdk, uninstallJdk, checkJdkUpdates,
  type JdkRuntimeInfo, type JdkProgress, type JdkUpdateInfo } from '../api/jdkRuntime'
import { listen } from '@tauri-apps/api/event'
import type { RuntimeProgress } from '../api/runtimeProgress'
import RuntimeProgressBar from '../components/RuntimeProgressBar'
import { getAppInfo, fetchNodeLatestLts, installToolkitFromZip, getToolchainCandidates, getToolVersion, type AppInfo, type ToolCandidate } from '../api/environment'
import { checkWithProxy, withProxy } from '../api/updateProxy'
import { useProjectStore } from '../stores/projectStore'

/** 版本号比较：v22.14.0 vs 22.13.0；返回 a-b 差值（>0 表示 a 新）。
 *  非数字段（如 git 的 windows 段）退化为字符串比较，兼容 Git for Windows 版本号。 */
function compareVersions(a: string, b: string): number {
  const pa = a.replace(/^v/i, '').split('.')
  const pb = b.replace(/^v/i, '').split('.')
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? ''
    const y = pb[i] ?? ''
    const nx = Number(x)
    const ny = Number(y)
    if (!Number.isNaN(nx) && !Number.isNaN(ny)) {
      if (nx !== ny) return nx - ny
    } else if (x !== y) {
      return x < y ? -1 : 1
    }
  }
  return 0
}

/** 可手动升级/选择目录的工具项（工程结构项除外） */
const TOOL_NAMES = ['hvigorw', 'hdc', 'ohpm']

/** 华为官方 Command Line Tools 下载页（需登录华为账号） */
const HARMONY_TOOLCHAIN_DOWNLOAD_URL =
  'https://developer.huawei.com/consumer/cn/download/command-line-tools-for-hmos'

export default function HealthPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const projectPath = useProjectStore((s) => s.currentProject?.path)
  const currentProject = useProjectStore((s) => s.currentProject)
  const [results, setResults] = useState<HealthResult[]>([])
  const [toolchain, setToolchain] = useState<ToolchainCheck[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // 自定义工具链目录（逗号/换行分隔多个），localStorage 持久化
  const [customPaths, setCustomPaths] = useState<string>(() => {
    try {
      return JSON.parse(localStorage.getItem('deveco-switch-toolchain-paths') || '""')
    } catch {
      return ''
    }
  })
  // Node 运行时（内置便携版兑底 + 在线升级）
  const [nodeRt, setNodeRt] = useState<NodeRuntimeInfo | null>(null)
  const [rtBusy, setRtBusy] = useState(false)
  const [rtMsg, setRtMsg] = useState<string | null>(null)
  const [rtVersion, setRtVersion] = useState('')
  // 最新 Node LTS（自动检查）
  const [nodeLatestLts, setNodeLatestLts] = useState<string | null>(null)
  const [ltsMsg, setLtsMsg] = useState<string | null>(null)
  // Git 运行时（内置便携版兑底 + 在线升级）
  const [gitRt, setGitRt] = useState<GitRuntimeInfo | null>(null)
  const [gitBusy, setGitBusy] = useState(false)
  const [gitMsg, setGitMsg] = useState<string | null>(null)
  const [gitLatest, setGitLatest] = useState<string | null>(null)
  // JDK 运行时（多版本并存 + 默认切换；系统无 JDK 时构建自动注入内置 JAVA_HOME）
  const [jdkRt, setJdkRt] = useState<JdkRuntimeInfo | null>(null)
  const [jdkBusy, setJdkBusy] = useState<string | null>(null)
  const [jdkMsg, setJdkMsg] = useState<string | null>(null)
  const [jdkFeature, setJdkFeature] = useState('17')
  const [jdkReleases, setJdkReleases] = useState<string[]>([])
  // JDK 下载默认自动（优先系统代理，无则直连）；勾选后强制走系统代理
  const [jdkUseProxy, setJdkUseProxy] = useState(false)
  // JDK 安装/更新进度（后端事件推送，不阻塞界面）与更新检查结果
  const [jdkProgress, setJdkProgress] = useState<JdkProgress | null>(null)
  const [jdkUpdates, setJdkUpdates] = useState<Record<string, JdkUpdateInfo>>({})
  // 应用基座（自身版本 / 检查更新）
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [updateInfo, setUpdateInfo] = useState<string | null>(null)
  const [updateAvail, setUpdateAvail] = useState(false)
  const [updating, setUpdating] = useState(false)
  // 工具链各工具版本（路径 → 版本串）
  const [toolVersions, setToolVersions] = useState<Record<string, string>>({})
  // 手动升级中的工具名
  const [toolkitBusy, setToolkitBusy] = useState<string | null>(null)
  // 环境选择下拉：当前展开的工具名、候选列表与加载状态
  const [candMenu, setCandMenu] = useState<string | null>(null)
  const [candidates, setCandidates] = useState<ToolCandidate[]>([])
  const [candLoading, setCandLoading] = useState(false)
  // 鸿蒙 SDK / command-line-tools 环境（后端持久化）
  const [harmonyEnv, setHarmonyEnv] = useState<HarmonyEnv | null>(null)
  const [envLoading, setEnvLoading] = useState(false)
  const [envSaving, setEnvSaving] = useState(false)
  const [envMsg, setEnvMsg] = useState<string | null>(null)
  const [sdkInput, setSdkInput] = useState('')
  const [cliInput, setCliInput] = useState('')
  // 当前工程的 SDK 版本对齐检查结果
  const [alignment, setAlignment] = useState<ProjectSdkAlignment | null>(null)
  // OpenHarmony 文档库（无需登录的离线 API 文档镜像）
  const [docsStatus, setDocsStatus] = useState<HarmonyDocsStatus | null>(null)
  const [docsBusy, setDocsBusy] = useState(false)
  const [docsMsg, setDocsMsg] = useState<string | null>(null)
  const [docQuery, setDocQuery] = useState('')
  const [docHits, setDocHits] = useState<DocEntry[] | null>(null)
  const [docSearching, setDocSearching] = useState(false)
  const [docDetail, setDocDetail] = useState<{ rel_path: string; title: string; text: string } | null>(null)
  // 文档库 / Node 下载是否走系统代理（默认自动：优先系统代理，无则直连；勾选后强制）
  const [docsUseProxy, setDocsUseProxy] = useState(false)
  const [nodeUseProxy, setNodeUseProxy] = useState(false)
  // Git 升级是否强制走系统代理（默认自动）
  const [gitUseProxy, setGitUseProxy] = useState(false)
  // Node / Git 升级进度（事件推送，与 JDK 一致）
  const [nodeProgress, setNodeProgress] = useState<RuntimeProgress | null>(null)
  const [gitProgress, setGitProgress] = useState<RuntimeProgress | null>(null)
  // 应用基座更新是否走系统代理（更新源在 GitHub，默认勾选）
  const [baseUseProxy, setBaseUseProxy] = useState(true)

  // 文档库状态 + 首轮加载
  useEffect(() => {
    getHarmonyDocsStatus().then(setDocsStatus).catch(() => setDocsStatus(null))
  }, [])

  const handleDocsSync = async () => {
    setDocsBusy(true)
    setDocsMsg(null)
    try {
      const st = await updateHarmonyDocs(true, docsUseProxy)
      setDocsStatus(st)
      setDocsMsg(t('health.docsSynced', { count: st.doc_count }))
    } catch (e) {
      setDocsMsg(t('health.docsSyncFailed', { err: String(e) }))
    } finally {
      setDocsBusy(false)
    }
  }

  const handleDocSearch = async () => {
    const q = docQuery.trim()
    if (!q) return
    setDocSearching(true)
    setDocDetail(null)
    try {
      setDocHits(await searchHarmonyDocs(q, 15))
    } catch (e) {
      setDocHits([])
      setDocsMsg(t('health.docsSearchFailed', { err: String(e) }))
    } finally {
      setDocSearching(false)
    }
  }

  const handleDocOpen = async (e: DocEntry) => {
    try {
      const text = await readHarmonyDoc(e.rel_path)
      setDocDetail({ rel_path: e.rel_path, title: e.title, text })
    } catch (err) {
      setDocsMsg(t('health.docsReadFailed', { err: String(err) }))
    }
  }

  const loadNodeRuntime = async () => {
    try {
      setNodeRt(await getNodeRuntime())
    } catch (e) {
      setRtMsg(t('health.nodeQueryFailed', { err: String(e) }))
    }
    // 自动检查最新 LTS（失败仅提示，不影响其他功能）
    try {
      setNodeLatestLts(await fetchNodeLatestLts())
      setLtsMsg(null)
    } catch (e) {
      setLtsMsg(t('health.ltsQueryFailed', { err: String(e) }))
    }
  }
  // 挂载时加载一次：函数引用每次渲染变化属预期，不加入依赖避免重复请求
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { loadNodeRuntime() }, [])

  const loadGitRuntime = async () => {
    try {
      setGitRt(await getGitRuntime())
      setGitMsg(null)
    } catch (e) {
      setGitMsg(t('health.gitQueryFailed', { err: String(e) }))
    }
    // 自动检查最新版（失败仅提示，不影响其他功能）
    try {
      setGitLatest(await fetchGitLatestVersion())
    } catch {
      setGitLatest(null)
    }
  }
  // 挂载时加载一次：函数引用每次渲染变化属预期，不加入依赖避免重复请求
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { loadGitRuntime() }, [])

  const loadJdkRuntime = async () => {
    try {
      setJdkRt(await getJdkRuntime())
      setJdkMsg(null)
    } catch (e) {
      setJdkMsg(t('health.jdkQueryFailed', { err: String(e) }))
    }
    // 自动查询可安装版本（失败静默，下拉保留默认 JDK 17）
    try {
      setJdkReleases(await fetchJdkReleases())
    } catch {
      setJdkReleases([])
    }
    // 自动检查已装版本更新（失败静默，不阻塞其他功能）
    try {
      const ups = await checkJdkUpdates()
      setJdkUpdates(Object.fromEntries(ups.map((u) => [u.feature, u])))
    } catch {
      setJdkUpdates({})
    }
  }
  // 挂载时加载一次：函数引用每次渲染变化属预期，不加入依赖避免重复请求
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { loadJdkRuntime() }, [])

  // 监听 Node / Git 运行时升级进度事件（与 JDK 共用 RuntimeProgressBar 展示）
  useEffect(() => {
    let un1: (() => void) | undefined
    let un2: (() => void) | undefined
    listen<RuntimeProgress>('node-runtime-progress', (ev) => setNodeProgress(ev.payload))
      .then((fn) => { un1 = fn })
      .catch(() => {})
    listen<RuntimeProgress>('git-runtime-progress', (ev) => setGitProgress(ev.payload))
      .then((fn) => { un2 = fn })
      .catch(() => {})
    return () => { un1?.(); un2?.() }
  }, [])

  // 监听 JDK 安装/更新进度事件（下载状态/网络检查，不阻塞界面）
  useEffect(() => {
    let unlisten: (() => void) | undefined
    listen<RuntimeProgress>('jdk-install-progress', (ev) => setJdkProgress(ev.payload))
      .then((fn) => { unlisten = fn })
      .catch(() => {})
    return () => { unlisten?.() }
  }, [])

  useEffect(() => {
    getAppInfo().then(setAppInfo).catch((e) => console.error(e))
  }, [])

  /** 加载已持久化的鸿蒙环境配置 */
  const loadHarmonyEnv = async () => {
    setEnvLoading(true)
    setEnvMsg(null)
    try {
      const env = await getHarmonyEnv()
      setHarmonyEnv(env)
      setSdkInput(env.sdk_root ?? '')
      setCliInput(env.cli?.root ?? '')
    } catch (e) {
      setEnvMsg(String(e))
    } finally {
      setEnvLoading(false)
    }
  }

  useEffect(() => {
    loadHarmonyEnv()
  }, [])

  // 工程 SDK 版本对齐：仅在有工程路径且是鸿蒙工程时检查
  useEffect(() => {
    if (!projectPath) return
    let cancelled = false
    checkProjectSdkAlignment(projectPath)
      .then((r) => {
        if (!cancelled) setAlignment(r)
      })
      .catch(() => {
        if (!cancelled) setAlignment(null)
      })
    return () => {
      cancelled = true
    }
  }, [projectPath, harmonyEnv?.default_api])

  /** 重新自动探测（忽略手动配置），仅用于预览"自动发现"结果 */
  const redetectEnv = async () => {
    setEnvLoading(true)
    setEnvMsg(null)
    try {
      const env = await detectHarmonyEnv()
      setHarmonyEnv(env)
      setSdkInput(env.sdk_root ?? '')
      setCliInput(env.cli?.root ?? '')
      setEnvMsg(env.sdk_root || env.cli ? t('health.envAutoDetected') : t('health.envNotFound'))
    } catch (e) {
      setEnvMsg(String(e))
    } finally {
      setEnvLoading(false)
    }
  }

  /** 选择目录 */
  const pickFolder = async (which: 'sdk' | 'cli') => {
    try {
      const selected = await open({ directory: true, multiple: false })
      if (typeof selected === 'string' && selected) {
        if (which === 'sdk') setSdkInput(selected)
        else setCliInput(selected)
      }
    } catch {
      // 用户取消
    }
  }

  /** 保存手动配置（后端校验 + 持久化 + 刷新子进程 PATH） */
  const saveEnv = async () => {
    setEnvSaving(true)
    setEnvMsg(null)
    try {
      const env = await saveHarmonyEnv(sdkInput.trim() || null, cliInput.trim() || null)
      setHarmonyEnv(env)
      setSdkInput(env.sdk_root ?? '')
      setCliInput(env.cli?.root ?? '')
      setEnvMsg(t('health.envSaved'))
    } catch (e) {
      setEnvMsg(String(e))
    } finally {
      setEnvSaving(false)
    }
  }

  /** 清除手动配置，回到自动探测 */
  const clearEnv = async () => {
    setEnvSaving(true)
    setEnvMsg(null)
    try {
      const env = await saveHarmonyEnv(null, null)
      setHarmonyEnv(env)
      setSdkInput('')
      setCliInput('')
      setEnvMsg(t('health.envReset'))
    } catch (e) {
      setEnvMsg(String(e))
    } finally {
      setEnvSaving(false)
    }
  }

  /** 读取各工具版本（并行，失败置 '-'，不阻塞展示） */
  const loadToolVersions = async (checks: ToolchainCheck[]) => {
    const entries = await Promise.all(
      checks
        .filter((c) => TOOL_NAMES.includes(c.name) && c.found)
        .map(async (c) => {
          try {
            return [c.detail, await getToolVersion(c.detail)] as const
          } catch {
            return [c.detail, t('health.versionReadFailed')] as const
          }
        }),
    )
    setToolVersions(Object.fromEntries(entries))
  }

  const rtSourceLabel = (s: string) =>
    s === 'system' ? t('health.srcSystem') : s === 'upgraded' ? t('health.srcUpgraded') : s === 'bundled' ? t('health.srcBundled') : t('health.srcNone')

  const handleUpgrade = async () => {
    const v = rtVersion.trim()
    if (!window.confirm(v ? t('health.upgradeConfirmV', { version: v }) : t('health.upgradeConfirm'))) return
    setRtBusy(true)
    setRtMsg(null)
    setNodeProgress(null)
    try {
      const info = await upgradeNodeRuntime(v || undefined, nodeUseProxy ? true : undefined)
      setNodeRt(info)
      setRtVersion('')
      setRtMsg(t('health.upgradeDone'))
    } catch (e) {
      setRtMsg(t('health.upgradeFailed', { err: String(e) }))
    }
    setRtBusy(false)
    setNodeProgress(null)
  }

  const handleReset = async () => {
    if (!window.confirm(t('health.resetConfirm'))) return
    setRtBusy(true)
    setRtMsg(null)
    try {
      setNodeRt(await resetNodeRuntime())
      setRtMsg(t('health.resetDone'))
    } catch (e) {
      setRtMsg(t('health.resetFailed', { err: String(e) }))
    }
    setRtBusy(false)
  }

  /** Git 运行时升级到最新版（后端自动查 GitHub 最新 tag，进度事件推送） */
  const handleGitUpgrade = async () => {
    const msg = gitLatest
      ? t('health.gitUpgradeConfirm', { version: gitLatest })
      : t('health.gitUpgradeConfirmNoVer')
    if (!window.confirm(msg)) return
    setGitBusy(true)
    setGitMsg(null)
    setGitProgress(null)
    try {
      const info = await upgradeGitRuntime(gitUseProxy ? true : undefined)
      setGitRt(info)
      setGitMsg(t('health.gitUpgradeDone'))
    } catch (e) {
      setGitMsg(t('health.upgradeFailed', { err: String(e) }))
    }
    setGitBusy(false)
    setGitProgress(null)
  }

  /** Git 运行时恢复出厂捆绑版 */
  const handleGitReset = async () => {
    if (!window.confirm(t('health.resetConfirm'))) return
    setGitBusy(true)
    setGitMsg(null)
    try {
      setGitRt(await resetGitRuntime())
      setGitMsg(t('health.resetDone'))
    } catch (e) {
      setGitMsg(t('health.resetFailed', { err: String(e) }))
    }
    setGitBusy(false)
  }

  /** JDK：在线安装/更新指定 feature 版本（下载约 190MB，进度事件推送，完成后立即生效） */
  const handleJdkInstall = async (feature: string, isUpdate: boolean) => {
    setJdkBusy(feature)
    setJdkMsg(null)
    setJdkProgress(null)
    try {
      const info = await installJdk(feature, jdkUseProxy ? true : undefined)
      setJdkRt(info)
      setJdkMsg(isUpdate
        ? t('health.jdkUpdated', { feature, version: jdkUpdates[feature]?.latest ?? '' })
        : t('health.jdkInstallDone', { feature }))
      // 刷新更新检查结果（该版本已是最新）
      try {
        const ups = await checkJdkUpdates()
        setJdkUpdates(Object.fromEntries(ups.map((u) => [u.feature, u])))
      } catch { /* 静默 */ }
    } catch (e) {
      setJdkMsg(t('health.upgradeFailed', { err: String(e) }))
    }
    setJdkBusy(null)
    setJdkProgress(null)
  }

  /** JDK：切换默认版本（多版本并存时切换构建/命令使用的 JDK） */
  const handleJdkSetDefault = async (feature: string) => {
    setJdkBusy(feature)
    setJdkMsg(null)
    try {
      const info = await setDefaultJdk(feature)
      setJdkRt(info)
      setJdkMsg(t('health.jdkSetDefaultDone', { feature }))
    } catch (e) {
      setJdkMsg(t('health.upgradeFailed', { err: String(e) }))
    }
    setJdkBusy(null)
  }

  /** JDK：卸载在线安装版（捆绑版不展示卸载按钮） */
  const handleJdkUninstall = async (feature: string) => {
    if (!window.confirm(t('health.jdkUninstallConfirm', { feature }))) return
    setJdkBusy(feature)
    setJdkMsg(null)
    try {
      const info = await uninstallJdk(feature)
      setJdkRt(info)
      setJdkMsg(t('health.jdkUninstallDone', { feature }))
    } catch (e) {
      setJdkMsg(t('health.upgradeFailed', { err: String(e) }))
    }
    setJdkBusy(null)
  }

  /** 基座：检查更新（按勾选决定是否走系统代理；更新源在 GitHub，默认走） */
  const checkUpdate = async () => {
    setCheckingUpdate(true)
    setUpdateInfo(null)
    try {
      const update = baseUseProxy ? await withProxy(checkWithProxy) : await checkWithProxy()
      setUpdateAvail(!!update)
      setUpdateInfo(update ? t('health.updateFound', { version: update.version }) : t('health.updateLatest'))
    } catch (e) {
      setUpdateAvail(false)
      setUpdateInfo(t('health.updateCheckFailed', { err: String(e) }))
    }
    setCheckingUpdate(false)
  }

  /** 基座：下载并安装更新 */
  const handleUpdate = async () => {
    setUpdating(true)
    try {
      const update = baseUseProxy ? await withProxy(checkWithProxy) : await checkWithProxy()
      if (update) {
        // 下载+安装同样置于代理窗口内（显式 proxy 已随 check 传入，环境变量注入为双保险）
        if (baseUseProxy) {
          await withProxy(async () => {
            await update.downloadAndInstall()
          })
        } else {
          await update.downloadAndInstall()
        }
      }
    } catch (e) {
      alert(t('health.updateFailed', { err: String(e) }))
    }
    setUpdating(false)
  }

  /** 为指定工具选择文件夹：把选中的目录追加到自定义工具链列表并重新检测 */
  const pickDirectoryFor = async () => {
    try {
      const picked = await open({ directory: true })
      if (!picked) return
      const existing = customPaths
        .split(/[\n,]/)
        .map((s) => s.trim().replace(/^"|"$/g, ''))
        .filter(Boolean)
      const next = existing.includes(picked) ? existing.join('\n') : [...existing, picked].join('\n')
      setCustomPaths(next)
      localStorage.setItem('deveco-switch-toolchain-paths', JSON.stringify(next))
      load()
    } catch (e) {
      alert(t('health.pickDirFailed', { err: String(e) }))
    }
  }

  /** 从本地 zip 安装工具包（官方 Command Line Tools 压缩包），解压到软件数据目录 */
  const handleImportZip = async (name: string) => {
    try {
      const picked = await open({ multiple: false, filters: [{ name: 'Zip', extensions: ['zip'] }] })
      if (!picked || typeof picked !== 'string') return
      setToolkitBusy(name)
      const dir = await installToolkitFromZip(name, picked)
      alert(t('health.toolkitImportDone', { name, dir }))
      load()
    } catch (e) {
      alert(t('health.toolkitImportFailed', { name, err: String(e) }))
    }
    setToolkitBusy(null)
  }

  /** 展开/收起某工具的候选环境目录菜单 */
  const toggleCandidates = async (name: string) => {
    if (candMenu === name) {
      setCandMenu(null)
      return
    }
    setCandMenu(name)
    setCandLoading(true)
    try {
      const paths = customPaths
        .split(/[\n,]/)
        .map((s) => s.trim().replace(/^"|"$/g, ''))
        .filter(Boolean)
      setCandidates(await getToolchainCandidates(name, paths))
    } catch {
      setCandidates([])
    }
    setCandLoading(false)
  }

  /** 选择候选目录：追加到自定义列表（最高优先级）并重新检测 */
  const chooseCandidate = (path: string) => {
    const existing = customPaths
      .split(/[\n,]/)
      .map((s) => s.trim().replace(/^"|"$/g, ''))
      .filter(Boolean)
    const next = existing.includes(path) ? existing.join('\n') : [...existing, path].join('\n')
    setCustomPaths(next)
    localStorage.setItem('deveco-switch-toolchain-paths', JSON.stringify(next))
    setCandMenu(null)
    load()
  }

  /** 候选来源的本地化标签 */
  const sourceLabel = (source: string) => {
    switch (source) {
      case 'bundled': return t('health.toolchainSourceBundled')
      case 'custom': return t('health.toolchainSourceCustom')
      case 'deveco': return t('health.toolchainSourceDeveco')
      case 'sdk': return t('health.toolchainSourceSdk')
      case 'path': return t('health.toolchainSourcePath')
      default: return source
    }
  }

  // 工程结构检查项的描述（有 structure 时前端拼 i18n 文案，不再用后端中文 detail）
  const structureDesc = (c: ToolchainCheck): string => {
    const s = c.structure!
    const pathLine = c.detail.split('\n')[0]
    if (s.kind === 'single') {
      return `${pathLine}\n${t('health.projectStructureSingle')}`
    }
    if (s.kind === 'workspace') {
      const names = s.projects.join(', ') + (s.projects.length < s.total ? ', …' : '')
      return `${pathLine}\n${t('health.projectStructureWorkspace', { names, total: s.total })}`
    }
    const reason = s.dir_exists ? t('health.projectStructureDirExists') : t('health.projectStructureDirMissing')
    return `${pathLine}\n${t('health.projectStructureInvalid', { missing: s.missing.join(', '), reason })}`
  }

  // 点击菜单外部时关闭候选下拉
  useEffect(() => {
    if (!candMenu) return
    const close = () => setCandMenu(null)
    document.addEventListener('click', close)
    return () => document.removeEventListener('click', close)
  }, [candMenu])

  const load = async () => {
    setLoading(true)
    setError(null)
    const paths = customPaths
      .split(/[\n,]/)
      .map((s) => s.trim().replace(/^"|"$/g, ''))
      .filter(Boolean)
    try {
      const r = await checkAllHealth()
      setResults(r)
    } catch (e) {
      setError(String(e))
    }
    // 鸿蒙工具链检查（hvigorw / hdc / ohpm / 工程结构），失败不影响主检查；
    // 绑定项目时传入项目 id 触发工程结构检查（全局模式无项目可查，后端不返回该项）
    try {
      const c = await checkHarmonyToolchain(currentProject?.id ?? undefined, paths)
      setToolchain(c)
      loadToolVersions(c)
    } catch (e) {
      console.error(e)
    }
    setLoading(false)
  }

  // 挂载时加载一次：函数引用每次渲染变化属预期，不加入依赖避免重复请求
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { load() }, [])

  const statusColor = (status: string) => {
    switch (status) {
      case 'healthy': return 'var(--success)'
      case 'degraded': return 'var(--warning)'
      case 'down': return 'var(--danger)'
      default: return 'var(--text-secondary)'
    }
  }

  const statusLabel = (status: string) => {
    switch (status) {
      case 'healthy': return t('health.healthy')
      case 'degraded': return t('health.degraded')
      case 'down': return t('health.down')
      default: return t('health.unknown')
    }
  }

  // Node 是否有可升级版本（当前生效版本 < 最新 LTS）
  const nodeUpgradable =
    nodeRt?.node_version && nodeLatestLts && compareVersions(nodeRt.node_version, nodeLatestLts) < 0

  // Git 当前生效版本号（去掉 "git version " 前缀）与可升级判断
  const gitVersionNum = gitRt?.git_version?.replace(/^git version\s*/i, '').trim() || ''
  const gitUpgradable =
    gitVersionNum && gitLatest && compareVersions(gitVersionNum, gitLatest) < 0

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-semibold">{t('health.envTitle')}</h2>
        <button
          onClick={load}
          disabled={loading}
          className="px-4 py-2 btn-primary rounded-lg text-sm  disabled:opacity-50 transition-colors"
        >
          {loading ? t('health.checking') : t('health.refresh')}
        </button>
      </div>

      <div className="space-y-3">
        {error && (
          <div className="rounded-lg border border-[var(--danger)]/25 bg-[var(--danger)]/10 px-4 py-3 text-[13px] text-[var(--danger)]">
            {t('health.checkFailed')}：{error}
          </div>
        )}
        {results.map((r) => (
          <div key={r.provider_id} className="modern-card rounded-lg p-4 flex items-center justify-between">
            <div className="flex items-center gap-3">
              <div
                className="w-3 h-3 rounded-full"
                style={{ backgroundColor: statusColor(r.status) }}
              />
              <div>
                <span className="font-medium">{r.provider_name}</span>
                <span className="text-xs text-[var(--text-secondary)] ml-2">{statusLabel(r.status)}</span>
              </div>
            </div>
            <div className="text-right">
              {r.latency_ms !== null && (
                <span className="text-sm font-mono tnum">{r.latency_ms}ms</span>
              )}
              {r.error && (
                <p className="text-xs text-[var(--danger)] mt-1">{r.error}</p>
              )}
            </div>
          </div>
        ))}
        {results.length === 0 && !loading && !error && (
          <div className="flex flex-col items-center gap-3 py-10 text-center">
            <p className="text-[var(--text-secondary)] text-sm">{t('health.noProvider')}</p>
            <button
              onClick={() => navigate('/providers')}
              className="h-9 px-4 rounded-lg btn-primary text-[13px] font-medium  active:scale-[0.98] transition-all"
            >
              {t('health.goAdd')}
            </button>
          </div>
        )}
      </div>

      {/* 鸿蒙 SDK / command-line-tools 环境：自动探测 + 手动指定（后端持久化） */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.harmonyEnvTitle')}</h3>
      <div className="modern-card rounded-lg p-4 mb-3">
        {/* 状态总览 */}
        <div className="flex items-center justify-between gap-3 flex-wrap mb-3">
          <div className="flex items-center gap-3 min-w-0">
            <div
              className="w-3 h-3 rounded-full shrink-0"
              style={{
                background: harmonyEnv?.sdk_root || harmonyEnv?.cli ? 'var(--success)' : 'var(--warning)',
              }}
            />
            <div className="min-w-0">
              <div className="text-[13px] font-medium text-[var(--text-primary)]">
                {harmonyEnv?.sdk_root || harmonyEnv?.cli
                  ? t('health.envDetected')
                  : t('health.envMissing')}
              </div>
              <div className="text-[11px] text-[var(--text-muted)] mt-0.5">
                {harmonyEnv?.source === 'manual' ? t('health.envSourceManual') : t('health.envSourceAuto')}
                {harmonyEnv?.studio_dir ? ` · DevEco Studio: ${harmonyEnv.studio_dir}` : ''}
              </div>
            </div>
          </div>
          <div className="flex gap-2 shrink-0">
            <button
              onClick={redetectEnv}
              disabled={envLoading || envSaving}
              className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] disabled:opacity-50 transition-colors"
            >
              {t('health.envRedetect')}
            </button>
            <button
              onClick={clearEnv}
              disabled={envLoading || envSaving}
              className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] disabled:opacity-50 transition-colors"
            >
              {t('health.envResetBtn')}
            </button>
          </div>
        </div>

        {/* 检测到的详细信息 */}
        {harmonyEnv && (
          <div className="text-xs text-[var(--text-muted)] font-mono break-all space-y-1 mb-3">
            {harmonyEnv.sdk_root && (
              <div>
                <span className="text-[var(--success)]">●</span> SDK: {harmonyEnv.sdk_root}
                {harmonyEnv.default_api && <span className="text-[var(--accent)]"> · 默认 API {harmonyEnv.default_api}</span>}
              </div>
            )}
            {harmonyEnv.sdk_variants.length > 0 && (
              <div className="pl-4 space-y-0.5">
                {harmonyEnv.sdk_variants.map((v) => {
                  const ets = v.components.find((c) => c.name === 'ets')
                  const compNames = v.components.map((c) => c.name).join('/')
                  return (
                    <div key={v.variant}>
                      <span className={v.is_default ? 'text-[var(--accent)]' : 'text-[var(--text-secondary)]'}>
                        {v.is_default ? '★' : '·'} {v.variant}
                      </span>
                      {v.api_version && <span className="ml-2">API {v.api_version}</span>}
                      {ets?.version && <span className="ml-2 text-[var(--text-muted)]">({ets.version})</span>}
                      <span className="ml-2 text-[var(--text-muted)]">[{compNames}]</span>
                      {ets?.api_dir && <span className="ml-2 text-[var(--success)]">api: 已索引</span>}
                    </div>
                  )
                })}
              </div>
            )}
            {harmonyEnv.cli && (
              <div>
                <span className="text-[var(--success)]">●</span> command-line-tools: {harmonyEnv.cli.root}
                <span className="ml-2">
                  {[
                    harmonyEnv.cli.has_hdc && 'hdc',
                    harmonyEnv.cli.has_ohpm && 'ohpm',
                    harmonyEnv.cli.has_hvigorw && 'hvigorw',
                  ].filter(Boolean).join(' / ') || '—'}
                </span>
              </div>
            )}
            {harmonyEnv.hdc_path && !harmonyEnv.cli?.has_hdc && (
              <div>
                <span className={harmonyEnv.hdc_source === 'path' ? 'text-[var(--warning)]' : 'text-[var(--success)]'}>
                  ●
                </span>{' '}
                hdc ({sourceLabel(harmonyEnv.hdc_source ?? 'sdk')}): {harmonyEnv.hdc_path}
                {harmonyEnv.hdc_source !== 'path' && (
                  <span className="ml-2 text-[10px] text-[var(--text-muted)]">{t('health.toolchainHdcFallback')}</span>
                )}
              </div>
            )}
          </div>
        )}

        {/* 未找到时的建议路径提示 */}
        {harmonyEnv && harmonyEnv.suggestions.length > 0 && (
          <div className="rounded-md bg-[var(--warning)]/8 border border-[var(--warning)]/20 px-3 py-2 mb-3">
            <div className="text-[11px] text-[var(--warning)] font-medium mb-1">{t('health.envSuggestions')}</div>
            <ul className="text-[11px] text-[var(--text-muted)] space-y-0.5 font-mono">
              {harmonyEnv.suggestions.slice(0, 6).map((s, i) => (
                <li key={i}>{s}</li>
              ))}
            </ul>
          </div>
        )}

        {/* 手动指定路径 */}
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <label className="text-[12px] text-[var(--text-secondary)] w-28 shrink-0">SDK 根目录</label>
            <input
              value={sdkInput}
              onChange={(e) => setSdkInput(e.target.value)}
              placeholder="C:\Program Files\Huawei\DevEco Studio\sdk"
              className="flex-1 h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-3 text-[12px] text-[var(--text-primary)] font-mono outline-none focus:border-[var(--accent)]"
            />
            <button
              onClick={() => pickFolder('sdk')}
              className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
            >
              {t('health.selectFolder')}
            </button>
          </div>
          <div className="flex items-center gap-2">
            <label className="text-[12px] text-[var(--text-secondary)] w-28 shrink-0">command-line-tools</label>
            <input
              value={cliInput}
              onChange={(e) => setCliInput(e.target.value)}
              placeholder="H:\command-line-tools"
              className="flex-1 h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-3 text-[12px] text-[var(--text-primary)] font-mono outline-none focus:border-[var(--accent)]"
            />
            <button
              onClick={() => pickFolder('cli')}
              className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
            >
              {t('health.selectFolder')}
            </button>
          </div>
          <div className="flex items-center gap-2 pt-1">
            <button
              onClick={saveEnv}
              disabled={envLoading || envSaving}
              className="h-8 px-4 rounded-lg btn-primary text-[12px] font-medium hover:opacity-90 disabled:opacity-50 transition-opacity"
            >
              {envSaving ? t('health.envSaving') : t('health.envSave')}
            </button>
            {envMsg && <span className="text-[11px] text-[var(--text-muted)]">{envMsg}</span>}
          </div>
        </div>
      </div>

      {/* 工程 SDK 版本对齐状态 */}
      {alignment && (
        <>
          <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.alignmentTitle')}</h3>
          <div
            className="rounded-lg border p-3 mb-3 flex items-center gap-3"
            style={{
              borderColor:
                alignment.status === 'ok' ? 'color-mix(in srgb, var(--success) 40%, transparent)'
                : alignment.status === 'behind' ? 'color-mix(in srgb, var(--danger) 40%, transparent)'
                : alignment.status === 'ahead' ? 'color-mix(in srgb, var(--success) 30%, transparent)'
                : 'var(--border)',
              background:
                alignment.status === 'behind' ? 'color-mix(in srgb, var(--danger) 6%, transparent)'
                : 'var(--bg-secondary)',
            }}
          >
            <div
              className="w-3 h-3 rounded-full shrink-0"
              style={{
                background:
                  alignment.status === 'ok' ? 'var(--success)'
                  : alignment.status === 'behind' ? 'var(--danger)'
                  : alignment.status === 'ahead' ? 'var(--success)'
                  : 'var(--warning)',
              }}
            />
            <div className="min-w-0">
              <div className="text-[13px] font-medium text-[var(--text-primary)]">{alignment.message}</div>
              <div className="text-[11px] text-[var(--text-muted)] mt-0.5 font-mono">
                {alignment.project_compatible && <span>工程: {alignment.project_compatible}</span>}
                {alignment.installed_api && <span className="ml-3">已装 SDK: API {alignment.installed_api}</span>}
              </div>
            </div>
          </div>
        </>
      )}

      {/* SDK API 浏览器 */}
      {harmonyEnv?.sdk_variants.some((v) => v.components.some((c) => c.api_dir)) && (
        <>
          <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.apiBrowserTitle')}</h3>
          <SdkApiBrowser />
        </>
      )}

      {/* OpenHarmony 文档库：无需登录的离线 API 文档 */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.docsTitle')}</h3>
      <div className="modern-card rounded-lg p-4 mb-3">
        <div className="flex items-center gap-3 flex-wrap">
          <div className={`w-3 h-3 rounded-full ${docsStatus?.downloaded ? 'bg-[var(--success)]' : 'bg-[var(--muted)]'} shrink-0`} />
          <span className="text-xs text-[var(--text-secondary)]">
            {docsStatus?.downloaded
              ? t('health.docsReady', { count: docsStatus.doc_count })
              : t('health.docsNotReady')}
          </span>
          <label className="ml-auto flex items-center gap-1.5 text-xs text-[var(--text-secondary)] cursor-pointer select-none shrink-0">
            <input
              type="checkbox"
              checked={docsUseProxy}
              onChange={(e) => setDocsUseProxy(e.target.checked)}
              className="accent-[var(--accent)]"
            />
            {t('health.useProxy')}
          </label>
          <button
            onClick={() => void handleDocsSync()}
            disabled={docsBusy}
            className="px-3 h-8 rounded-lg btn-primary text-xs font-medium  disabled:opacity-50 transition-colors"
          >
            {docsBusy ? t('health.docsSyncing') : (docsStatus?.downloaded ? t('health.docsUpdate') : t('health.docsDownload'))}
          </button>
        </div>
        <p className="text-xs text-[var(--text-muted)] mt-2 leading-relaxed">
          {t('health.docsDesc')}
        </p>
        {docsMsg && <p className="text-xs text-[var(--text-secondary)] mt-2 break-all">{docsMsg}</p>}
        {docsStatus?.root && (
          <p className="text-[11px] text-[var(--text-muted)] mt-1 font-mono break-all">{docsStatus.root}</p>
        )}
        {docsStatus?.downloaded && (
          <div className="mt-3 border-t border-[var(--border)] pt-3">
            <div className="flex gap-2">
              <input
                value={docQuery}
                onChange={(e) => setDocQuery(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && void handleDocSearch()}
                placeholder={t('health.docsSearchPlaceholder')}
                className="flex-1 h-8 px-3 rounded-lg modern-card border-[var(--border)] text-xs outline-none focus:border-[var(--accent)]"
              />
              <button
                onClick={() => void handleDocSearch()}
                disabled={docSearching || !docQuery.trim()}
                className="px-3 h-8 rounded-lg modern-card text-[var(--text-primary)] text-xs hover:border-[var(--accent)]/50 disabled:opacity-50 transition-colors"
              >
                {docSearching ? t('health.docsSearching') : t('health.docsSearch')}
              </button>
            </div>
            {docHits && docHits.length === 0 && (
              <p className="text-xs text-[var(--text-muted)] mt-2">{t('health.docsNoHits')}</p>
            )}
            {docHits && docHits.length > 0 && (
              <div className="mt-2 max-h-64 overflow-y-auto rounded-lg border border-[var(--border)] divide-y divide-[var(--border)]">
                {docHits.map((d) => (
                  <button
                    key={d.rel_path}
                    onClick={() => void handleDocOpen(d)}
                    className="w-full text-left px-3 py-2 hover:bg-[var(--bg-hover)] transition-colors"
                  >
                    <div className="flex items-center gap-2">
                      <span className="text-xs font-medium text-[var(--text-primary)] truncate">{d.title}</span>
                      {d.has_example && <span className="shrink-0 text-[10px] text-[var(--accent)]">📎</span>}
                    </div>
                    <div className="text-[10px] text-[var(--text-muted)] truncate mt-0.5">
                      [{d.kit || t('health.docsKitGeneral')}] {d.preview}
                    </div>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* 文档详情浮层 */}
      {docDetail && (
        <div
          className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm flex items-center justify-center p-6"
          onClick={() => setDocDetail(null)}
        >
          <div
            className="relative w-full max-w-3xl max-h-[80vh] rounded-xl overflow-hidden modern-card border-[var(--border)] shadow-2xl flex flex-col"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center gap-2 px-4 py-3 border-b border-[var(--border)] shrink-0">
              <span className="text-sm font-medium text-[var(--text-primary)] truncate">{docDetail.title}</span>
              <button
                onClick={() => setDocDetail(null)}
                className="ml-auto p-1 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]"
              >
                ✕
              </button>
            </div>
            <div className="flex-1 overflow-y-auto p-4">
              <p className="text-[11px] text-[var(--text-muted)] font-mono mb-3 break-all">{docDetail.rel_path}</p>
              <pre className="text-xs leading-relaxed text-[var(--text-secondary)] whitespace-pre-wrap break-all">{docDetail.text}</pre>
            </div>
          </div>
        </div>
      )}

      {/* 应用基座：自身版本 / 安装位置 / 检查更新（更新源在 GitHub，可勾选走系统代理） */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.baseTitle')}</h3>
      <div className="modern-card rounded-lg p-4 mb-3">
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <div className="flex items-center gap-3 min-w-0">
            <div className="w-3 h-3 rounded-full bg-[var(--success)] shrink-0" />
            <div className="min-w-0">
              <span className="font-medium">DevEco Switch</span>
              <span className="text-xs text-[var(--text-secondary)] ml-2">
                v{appInfo?.version ?? '...'}
              </span>
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <label className="flex items-center gap-1.5 text-xs text-[var(--text-secondary)] cursor-pointer select-none shrink-0">
              <input
                type="checkbox"
                checked={baseUseProxy}
                onChange={(e) => setBaseUseProxy(e.target.checked)}
                className="accent-[var(--accent)]"
              />
              {t('health.useProxy')}
            </label>
            <button
              onClick={checkUpdate}
              disabled={checkingUpdate}
              className="px-3 h-8 rounded-lg modern-card text-[var(--text-primary)] text-xs hover:border-[var(--accent)]/50 disabled:opacity-50 transition-colors"
            >
              {checkingUpdate ? t('health.checking') : t('health.checkUpdate')}
            </button>
            {updateAvail && (
              <button
                onClick={handleUpdate}
                disabled={updating}
                className="px-3 h-8 rounded-lg btn-primary text-xs font-medium  disabled:opacity-50 transition-colors"
              >
                {updating ? t('health.updating') : t('health.updateNow')}
              </button>
            )}
          </div>
        </div>
        {updateInfo && <p className="text-xs mt-2 break-all">{updateInfo}</p>}
        <p className="text-xs text-[var(--text-muted)] mt-2 font-mono break-all">
          {t('health.updateSource', { url: 'https://github.com/like3213934360-lab/Deveco-code-swich/releases/latest/download/latest.json' })}
        </p>
        {appInfo && (
          <div className="text-xs text-[var(--text-muted)] mt-2 font-mono break-all space-y-0.5">
            <p>{t('health.installDir', { dir: appInfo.install_dir || t('health.unknown') })}</p>
            <p>{t('health.dataDir', { dir: appInfo.data_dir || t('health.unknown') })}</p>
          </div>
        )}
      </div>

      {/* Node 运行时：系统未安装 npx 时内置便携版兑底，支持在线升级 */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.nodeRuntime')}</h3>
      <div className="modern-card rounded-lg p-4 mb-3">
        {nodeRt ? (
          <>
            <div className="flex items-center justify-between gap-3 flex-wrap">
              <div className="flex items-center gap-3 min-w-0">
                <div
                  className="w-3 h-3 rounded-full shrink-0"
                  style={{ backgroundColor: nodeRt.node_version ? 'var(--success)' : 'var(--danger)' }}
                />
                <div className="min-w-0">
                  <span className="font-medium">{rtSourceLabel(nodeRt.source)}</span>
                  <span className="text-xs text-[var(--text-secondary)] ml-2">
                    node {nodeRt.node_version || t('health.unavailable')} / npx {nodeRt.npx_version || t('health.unavailable')}
                  </span>
                  {nodeLatestLts && (
                    <span className="text-xs text-[var(--text-muted)] ml-2">{t('health.latestLts', { version: nodeLatestLts })}</span>
                  )}
                </div>
              </div>
              {nodeUpgradable && (
                <span className="px-2 py-0.5 rounded bg-[var(--warning)]/15 text-[var(--warning)] text-xs shrink-0">
                  {t('health.upgradable')}
                </span>
              )}
              {nodeRt.upgraded_dir && (
                <button
                  onClick={handleReset}
                  disabled={rtBusy}
                  className="px-3 h-8 rounded-lg border border-[var(--danger)]/40 text-[var(--danger)] text-xs hover:bg-[var(--danger)]/10 disabled:opacity-50 transition-colors shrink-0"
                >
                  {t('health.reset')}
                </button>
              )}
            </div>
            {nodeRt.dir && (
              <p className="text-xs text-[var(--text-muted)] mt-2 font-mono break-all">{t('health.activeDir', { dir: nodeRt.dir })}</p>
            )}
            {nodeRt.node_error && (
              <p className="text-xs text-[var(--danger)] mt-1 break-all">{nodeRt.node_error}</p>
            )}
            {nodeRt.npx_error && (
              <p className="text-xs text-[var(--danger)] mt-1 break-all">{nodeRt.npx_error}</p>
            )}
            {nodeRt.source === 'none' && (
              <p className="text-xs text-[var(--warning)] mt-2">{t('health.noNodeWarning')}</p>
            )}
            {nodeRt.source === 'system' && nodeRt.upgraded_dir && (
              <p className="text-xs text-[var(--text-muted)] mt-2">{t('health.systemNodeFallback')}</p>
            )}
            {ltsMsg && <p className="text-xs text-[var(--text-muted)] mt-2 break-all">{ltsMsg}</p>}
            <div className="flex gap-2 mt-3">
              <input
                value={rtVersion}
                onChange={(e) => setRtVersion(e.target.value)}
                onKeyDown={(e) => { if (e.key === 'Enter') handleUpgrade() }}
                placeholder={t('health.versionPlaceholder', { example: nodeLatestLts || '22.14.0' })}
                className="flex-1 px-3 h-8 rounded-lg modern-card border-[var(--border)] text-[12px] font-mono outline-none placeholder:text-[var(--text-muted)] focus:border-[var(--accent)] transition-colors"
              />
              <button
                onClick={handleUpgrade}
                disabled={rtBusy}
                className="px-4 h-8 rounded-lg btn-primary text-xs font-medium  disabled:opacity-50 transition-colors shrink-0"
              >
                {rtBusy ? t('health.upgrading') : t('health.upgrade')}
              </button>
              <label className="flex items-center gap-1.5 text-xs text-[var(--text-secondary)] cursor-pointer select-none shrink-0" title={t('health.autoProxyHint')}>
                <input
                  type="checkbox"
                  checked={nodeUseProxy}
                  onChange={(e) => setNodeUseProxy(e.target.checked)}
                  className="accent-[var(--accent)]"
                />
                {t('health.forceProxy')}
              </label>
            </div>
            {nodeProgress && <RuntimeProgressBar progress={nodeProgress} />}
            {rtMsg && <p className="text-xs mt-2 break-all">{rtMsg}</p>}
          </>
        ) : (
          <p className="text-sm text-[var(--text-secondary)]">{t('common.loading')}</p>
        )}
      </div>

      {/* Git 运行时：系统未装 Git 时内置便携版兑底，支持在线升级 */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.gitRuntime')}</h3>
      <div className="modern-card rounded-lg p-4 mb-3">
        {gitRt ? (
          <>
            <div className="flex items-center justify-between gap-3 flex-wrap">
              <div className="flex items-center gap-3 min-w-0">
                <div
                  className="w-3 h-3 rounded-full shrink-0"
                  style={{ backgroundColor: gitRt.git_version ? 'var(--success)' : 'var(--danger)' }}
                />
                <div className="min-w-0">
                  <span className="font-medium">
                    {gitRt.source === 'system' ? t('health.gitSrcSystem') : rtSourceLabel(gitRt.source)}
                  </span>
                  <span className="text-xs text-[var(--text-secondary)] ml-2">
                    {gitRt.git_version || t('health.unavailable')}
                  </span>
                  {gitLatest && (
                    <span className="text-xs text-[var(--text-muted)] ml-2">{t('health.gitLatest', { version: gitLatest })}</span>
                  )}
                </div>
              </div>
              {gitUpgradable && (
                <span className="px-2 py-0.5 rounded bg-[var(--warning)]/15 text-[var(--warning)] text-xs shrink-0">
                  {t('health.upgradable')}
                </span>
              )}
            </div>
            {gitRt.dir && (
              <p className="text-xs text-[var(--text-muted)] mt-2 font-mono break-all">{t('health.activeDir', { dir: gitRt.dir })}</p>
            )}
            {gitRt.git_error && (
              <p className="text-xs text-[var(--danger)] mt-1 break-all">{gitRt.git_error}</p>
            )}
            {gitRt.source === 'none' && (
              <p className="text-xs text-[var(--warning)] mt-2">{t('health.noGitWarning')}</p>
            )}
            <div className="flex items-center gap-2 mt-3">
              <button
                onClick={handleGitUpgrade}
                disabled={gitBusy}
                className="px-4 h-8 rounded-lg btn-primary text-xs font-medium  disabled:opacity-50 transition-colors shrink-0"
              >
                {gitBusy ? t('health.upgrading') : t('health.upgrade')}
              </button>
              <label className="flex items-center gap-1.5 text-xs text-[var(--text-secondary)] cursor-pointer select-none shrink-0" title={t('health.autoProxyHint')}>
                <input
                  type="checkbox"
                  checked={gitUseProxy}
                  onChange={(e) => setGitUseProxy(e.target.checked)}
                  className="accent-[var(--accent)]"
                />
                {t('health.forceProxy')}
              </label>
              {gitRt.upgraded_dir && (
                <button
                  onClick={handleGitReset}
                  disabled={gitBusy}
                  className="px-3 h-8 rounded-lg border border-[var(--danger)]/40 text-[var(--danger)] text-xs hover:bg-[var(--danger)]/10 disabled:opacity-50 transition-colors shrink-0"
                >
                  {t('health.reset')}
                </button>
              )}
            </div>
            {gitProgress && <RuntimeProgressBar progress={gitProgress} />}
            {gitMsg && <p className="text-xs mt-2 break-all">{gitMsg}</p>}
          </>
        ) : (
          <p className="text-sm text-[var(--text-secondary)]">{gitMsg ?? t('common.loading')}</p>
        )}
      </div>

      {/* JDK 运行时：多版本并存 + 默认切换；系统无 JDK 时构建自动注入内置 JAVA_HOME */}
      <h3 className="text-sm font-medium text-[var(--text-secondary)] mt-8 mb-3">{t('health.jdkRuntime')}</h3>
      <div className="modern-card rounded-lg p-4 mb-3">
        {jdkRt ? (
          <>
            <div className="flex items-center justify-between gap-3 flex-wrap">
              <div className="flex items-center gap-3 min-w-0">
                <div
                  className="w-3 h-3 rounded-full shrink-0"
                  style={{ backgroundColor: jdkRt.active_version ? 'var(--success)' : 'var(--danger)' }}
                />
                <div className="min-w-0">
                  <span className="font-medium">JDK {jdkRt.active_version || t('health.unavailable')}</span>
                  {jdkRt.system_java_home && (
                    <span className="text-xs text-[var(--text-muted)] ml-2 break-all">
                      {t('health.jdkSystemJavaHome', { path: jdkRt.system_java_home })}
                    </span>
                  )}
                </div>
              </div>
            </div>
            {jdkRt.active_dir && (
              <p className="text-xs text-[var(--text-muted)] mt-2 font-mono break-all">{t('health.activeDir', { dir: jdkRt.active_dir })}</p>
            )}
            {jdkRt.versions.length === 0 && (
              <p className="text-xs text-[var(--warning)] mt-2">{t('health.jdkNoWarning')}</p>
            )}
            {jdkRt.versions.length > 0 && (
              <div className="mt-3 space-y-2">
                {jdkRt.versions.map((v) => (
                  <div
                    key={`${v.source}-${v.feature}`}
                    className="flex items-center justify-between gap-2 flex-wrap modern-card rounded-lg px-3 py-2"
                  >
                    <div className="flex items-center gap-2 min-w-0">
                      {v.is_default && (
                        <span className="px-1.5 py-0.5 rounded bg-[var(--accent)]/15 text-[var(--accent)] text-[10px] shrink-0">
                          {t('health.jdkDefault')}
                        </span>
                      )}
                      <span className="font-mono text-xs">JDK {v.full_version || v.feature}</span>
                      <span className="text-[10px] text-[var(--text-muted)] shrink-0">
                        {v.source === 'bundled' ? t('health.jdkSrcBundled') : t('health.jdkSrcUpgraded')}
                      </span>
                    </div>
                    <div className="flex items-center gap-1.5 shrink-0">
                      {jdkUpdates[v.feature]?.updatable && (
                        <button
                          onClick={() => handleJdkInstall(v.feature, true)}
                          disabled={jdkBusy !== null}
                          className="px-2.5 h-7 rounded-lg border border-[var(--accent)]/40 text-[var(--accent)] text-[11px] hover:bg-[var(--accent)]/10 disabled:opacity-50 transition-colors"
                        >
                          {jdkBusy === v.feature
                            ? t('health.jdkUpdating')
                            : t('health.jdkUpdateTo', { version: jdkUpdates[v.feature].latest })}
                        </button>
                      )}
                      {!v.is_default && (
                        <button
                          onClick={() => handleJdkSetDefault(v.feature)}
                          disabled={jdkBusy !== null}
                          className="px-2.5 h-7 rounded-lg border border-[var(--accent)]/40 text-[var(--accent)] text-[11px] hover:bg-[var(--accent)]/10 disabled:opacity-50 transition-colors"
                        >
                          {t('health.jdkSetDefault')}
                        </button>
                      )}
                      {v.source === 'upgraded' && (
                        <button
                          onClick={() => handleJdkUninstall(v.feature)}
                          disabled={jdkBusy !== null}
                          className="px-2.5 h-7 rounded-lg border border-[var(--danger)]/40 text-[var(--danger)] text-[11px] hover:bg-[var(--danger)]/10 disabled:opacity-50 transition-colors"
                        >
                          {t('health.jdkUninstall')}
                        </button>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
            <div className="flex items-center gap-2 mt-3">
              <select
                value={jdkFeature}
                onChange={(e) => setJdkFeature(e.target.value)}
                className="px-2 h-8 rounded-lg modern-card border-[var(--border)] text-[12px] font-mono outline-none focus:border-[var(--accent)] transition-colors"
              >
                {(jdkReleases.length > 0 ? jdkReleases : ['17']).map((r) => (
                  <option key={r} value={r}>
                    JDK {r}{r === '17' ? ` (${t('health.jdkRecommended')})` : ''}
                  </option>
                ))}
              </select>
              <button
                onClick={() => handleJdkInstall(jdkFeature, false)}
                disabled={jdkBusy !== null}
                className="px-4 h-8 rounded-lg btn-primary text-xs font-medium  disabled:opacity-50 transition-colors shrink-0"
              >
                {jdkBusy === jdkFeature ? t('health.jdkInstalling') : t('health.jdkInstall')}
              </button>
              <label className="flex items-center gap-1.5 text-xs text-[var(--text-secondary)] cursor-pointer select-none shrink-0" title={t('health.autoProxyHint')}>
                <input
                  type="checkbox"
                  checked={jdkUseProxy}
                  onChange={(e) => setJdkUseProxy(e.target.checked)}
                  className="accent-[var(--accent)]"
                />
                {t('health.forceProxy')}
              </label>
            </div>
            {/* 下载状态 / 网络检查 / 解压进度（事件推送，不阻塞界面） */}
            {jdkProgress && <RuntimeProgressBar progress={jdkProgress} />}
            <p className="text-[11px] text-[var(--text-muted)] mt-2">{t('health.jdkSourceHint')}</p>
            {jdkMsg && <p className="text-xs mt-2 break-all">{jdkMsg}</p>}
          </>
        ) : (
          <p className="text-sm text-[var(--text-secondary)]">{t('common.loading')}</p>
        )}
      </div>

      {/* 鸿蒙工具链：hvigorw / hdc / ohpm / 工程结构 */}
      <div className="flex items-center justify-between mt-8 mb-3">
        <h3 className="text-sm font-medium text-[var(--text-secondary)]">{t('health.toolchainTitle')}</h3>
        <div className="flex items-center gap-2">
          <button
            onClick={() => shellOpen(HARMONY_TOOLCHAIN_DOWNLOAD_URL)}
            className="flex items-center gap-1.5 px-3 h-7 rounded-lg border border-[var(--accent)]/40 text-[var(--accent)] text-[11px] hover:bg-[var(--accent)]/10 transition-colors"
            title={t('health.toolchainOfficialTitle')}
          >
            <Icon name="download" size={12} />
            {t('health.toolchainOfficialDownload')}
          </button>
        </div>
      </div>
      {/* 自定义工具链目录：不依赖系统 PATH，直接指定 DevEco Studio 安装目录（手动填写） */}
      <div className="modern-card rounded-lg p-3 mb-3">
        <label className="block text-xs text-[var(--text-secondary)] mb-1.5">
          {t('health.customPathsLabel')}
        </label>
        <div className="flex gap-2">
          <textarea
            rows={2}
            value={customPaths}
            onChange={(e) => {
              setCustomPaths(e.target.value)
              localStorage.setItem('deveco-switch-toolchain-paths', JSON.stringify(e.target.value))
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                e.preventDefault()
                load()
              }
            }}
            placeholder={'例如：\nC:\\Program Files\\Huawei\\DevEco Studio\\tools\\hvigor\\bin\nC:\\Program Files\\Huawei\\DevEco Studio\\sdk\\default\\openharmony\\toolchains\nC:\\Program Files\\Huawei\\DevEco Studio\\tools\\ohpm\\bin'}
            className="flex-1 px-3 py-2 modern-card rounded-lg text-[12px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)] resize-none"
          />
          <button
            onClick={load}
            className="self-end px-4 h-9 rounded-lg btn-primary text-[13px] font-medium  transition-colors shrink-0"
          >
            {t('health.recheck')}
          </button>
        </div>
      </div>
      <div className="space-y-2">
        {toolchain.length === 0 && !loading ? (
          <p className="text-sm text-[var(--text-secondary)]">{t('health.toolchainEmpty')}</p>
        ) : (
          toolchain.map((c) => (
            <div
              key={c.name}
              className="modern-card rounded-lg p-3 flex items-start justify-between gap-3"
            >
              <div className="flex items-start gap-3 min-w-0">
                <div
                  className="w-3 h-3 rounded-full mt-1 shrink-0"
                  style={{ backgroundColor: c.found ? 'var(--success)' : 'var(--danger)' }}
                />
                <div className="min-w-0">
                  <span className="font-medium text-sm">
                    {c.name === 'project_structure' ? t('health.projectStructure') : c.name}
                  </span>
                  {c.found && toolVersions[c.detail] && (
                    <span className="text-xs text-[var(--text-secondary)] ml-2">
                      {t('health.version', { version: toolVersions[c.detail].slice(0, 40) })}
                    </span>
                  )}
                  <p className="text-xs text-[var(--text-secondary)] mt-0.5 break-all whitespace-pre-line">
                    {c.structure ? structureDesc(c) : c.detail}
                  </p>
                  {c.suggestion && !c.structure && (
                    <p className="text-xs text-[var(--warning)] mt-0.5">{c.suggestion}</p>
                  )}
                  {c.structure && c.structure.kind === 'invalid' && (
                    <p className="text-xs text-[var(--warning)] mt-0.5">{t('health.projectStructureInvalidHint')}</p>
                  )}
                </div>
              </div>
              {TOOL_NAMES.includes(c.name) && (
                <div className="relative flex flex-col gap-1.5 shrink-0 items-end">
                  <button
                    onClick={(e) => {
                      e.stopPropagation()
                      toggleCandidates(c.name)
                    }}
                    className={`px-3 h-7 rounded-lg border text-[11px] transition-colors ${
                      candMenu === c.name
                        ? 'border-[var(--accent)] bg-[var(--accent)]/10 text-[var(--accent)]'
                        : 'border-[var(--border)] bg-[var(--bg-card)] text-[var(--text-primary)] hover:border-[var(--accent)]/50'
                    }`}
                    title={t('health.pickDirTitle', { name: c.name })}
                  >
                    {t('health.pickDir')}
                  </button>
                  <button
                    onClick={() => handleImportZip(c.name)}
                    disabled={toolkitBusy !== null}
                    className="px-3 h-7 rounded-lg border border-[var(--accent)]/40 text-[var(--accent)] text-[11px] hover:bg-[var(--accent)]/10 disabled:opacity-50 transition-colors"
                    title={t('health.toolkitImportTitle', { name: c.name })}
                  >
                    {toolkitBusy === c.name ? t('health.upgrading') : t('health.toolkitImportZip')}
                  </button>
                  {candMenu === c.name && (
                    <div
                      onClick={(e) => e.stopPropagation()}
                      className="absolute right-0 top-full mt-1 z-20 w-80 modern-card rounded-lg shadow-lg overflow-hidden"
                    >
                      <div className="px-3 py-2 text-[10px] text-[var(--text-muted)] border-b border-[var(--border)]">
                        {t('health.toolchainCandidates', { name: c.name })}
                      </div>
                      <div className="max-h-56 overflow-y-auto">
                        {candLoading ? (
                          <p className="px-3 py-2 text-xs text-[var(--text-muted)]">{t('common.loading')}</p>
                        ) : candidates.length === 0 ? (
                          <p className="px-3 py-2 text-xs text-[var(--text-muted)]">{t('health.toolchainNoCandidate')}</p>
                        ) : (
                          candidates.map((cd) => (
                            <button
                              key={cd.path}
                              onClick={() => chooseCandidate(cd.path)}
                              className="w-full text-left px-3 py-2 hover:bg-[var(--bg-hover)] border-b border-[var(--border)]/50 transition-colors"
                            >
                              <span className="font-mono text-[11px] break-all text-[var(--text-primary)]">{cd.path}</span>
                              <span className="inline-block ml-1.5 shrink-0 text-[10px] px-1.5 py-0.5 rounded bg-[var(--bg-hover)] text-[var(--text-muted)]">
                                {sourceLabel(cd.source)}
                              </span>
                            </button>
                          ))
                        )}
                      </div>
                      <button
                        onClick={() => {
                          setCandMenu(null)
                          pickDirectoryFor()
                        }}
                        className="w-full text-left px-3 py-2 text-xs text-[var(--accent)] hover:bg-[var(--bg-hover)] border-t border-[var(--border)] transition-colors"
                      >
                        {t('health.toolchainBrowse')}
                      </button>
                    </div>
                  )}
                </div>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  )
}





