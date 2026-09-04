import { Routes, Route, NavLink, useNavigate } from 'react-router-dom'
import { lazy, Suspense, useEffect, useLayoutEffect, useState } from 'react'
import { getVersion } from '@tauri-apps/api/app'
import { useTranslation } from 'react-i18next'
import Icon, { type IconName } from './icons/Icon'
import UpdateChecker from './components/UpdateChecker'
import DesktopNotifyToast from './components/DesktopNotifyToast'
import PerfMonitor from './components/PerfMonitor'
import NotificationBell from './components/NotificationBell'
import LangToggle from './components/LangToggle'
import { ErrorBoundary } from './components/ErrorBoundary'
import { recordBootstrapStage } from './utils/perfTrace'
import { useThemeStore } from './stores/themeStore'
import { detectSystemLocale } from './api/desktop'
import { LANG_STORAGE_KEY } from './i18n'
import { detectGpu, getTierClass } from './utils/gpuDetect'
import { getItem } from './utils/storage'

// 页面级拆包：避免 Recharts、Markdown/KaTeX、设备诊断等全部阻塞首个可交互帧。
// Tauri 资源来自本地，懒加载没有网络不确定性，只把 JS 解析/执行摊到实际进入页面时。
const Home = lazy(() => import('./pages/Home'))
const LanPage = lazy(() => import('./pages/LanPage'))
const ProvidersPage = lazy(() => import('./pages/ProvidersPage'))
const VersionsPage = lazy(() => import('./pages/VersionsPage'))
const ConfigPage = lazy(() => import('./pages/ConfigPage'))
const LimitsPage = lazy(() => import('./pages/LimitsPage'))
const CostPage = lazy(() => import('./pages/CostPage'))
const McpPage = lazy(() => import('./pages/McpPage'))
const SkillsPage = lazy(() => import('./pages/SkillsPage'))
const KnowledgePage = lazy(() => import('./pages/KnowledgePage'))
const ApiKnowledgePage = lazy(() => import('./pages/ApiKnowledgePage'))
const HealthPage = lazy(() => import('./pages/HealthPage'))
const OhpmPage = lazy(() => import('./pages/OhpmPage'))
const TeamSharingPage = lazy(() => import('./pages/TeamSharingPage'))
const ReproductionBundlesPage = lazy(() => import('./pages/ReproductionBundlesPage'))
const ProxyPage = lazy(() => import('./pages/ProxyPage'))

const navItems: { path: string; labelKey: string; icon: IconName }[] = [
  { path: '/lan', labelKey: 'nav.lan', icon: 'devices' },
  { path: '/providers', labelKey: 'nav.provider', icon: 'bolt' },
  { path: '/versions', labelKey: 'nav.version', icon: 'package' },
  { path: '/config', labelKey: 'nav.config', icon: 'settings' },
  { path: '/limits', labelKey: 'nav.limits', icon: 'tune' },
  { path: '/cost', labelKey: 'nav.cost', icon: 'payments' },
  { path: '/proxy', labelKey: 'nav.proxy', icon: 'proxy' },
  { path: '/mcp', labelKey: 'nav.mcp', icon: 'mcp' },
  { path: '/skills', labelKey: 'nav.skill', icon: 'skill' },
  { path: '/team-sharing', labelKey: 'nav.teamSharing', icon: 'language' },
  { path: '/reproduction-bundles', labelKey: 'nav.reproductionBundles', icon: 'archive' },
  { path: '/knowledge', labelKey: 'nav.knowledge', icon: 'lightbulb' },
  { path: '/api-knowledge', labelKey: 'nav.apiKnowledge', icon: 'terminal' },
  { path: '/health', labelKey: 'nav.health', icon: 'health' },
  { path: '/ohpm', labelKey: 'nav.ohpm', icon: 'apps' },
]

export default function App() {
  const { i18n } = useTranslation()
  const [, setSysTick] = useState(0)

  // GPU 能力检测：应用启动时 useLayoutEffect 同步执行（canvas 不需要挂载到 DOM），
  // 在首帧绘制前将渲染分级挂到 <html> 元素上，CSS 可据此启停特效，JS 侧也能读取分级参数。
  // useLayoutEffect 在 DOM 变更后、绘制前运行，避免首帧出现无 class 状态导致闪烁
  useLayoutEffect(() => {
    const info = detectGpu()
    const cls = getTierClass(info.tier)
    const html = document.documentElement
    html.classList.remove('render-tier-high', 'render-tier-medium', 'render-tier-low')
    html.classList.add(cls)
    html.dataset.renderTier = info.tier
    html.dataset.gpuRenderer = info.renderer || 'unknown'
    html.dataset.platform = info.platform
    recordBootstrapStage('gpu-detected')
  }, [])

  // 语言"跟随系统"：设置为 auto 时，启动后异步探测系统语言并应用
  useEffect(() => {
    const saved = getItem(LANG_STORAGE_KEY)
    if (saved !== 'auto') {
      setSysTick((n) => n + 1)
      return
    }
    let cancelled = false
    detectSystemLocale()
      .then((info) => {
        if (cancelled) return
        const target = info.is_zh ? 'zh' : 'en'
        if (i18n.language !== target) {
          i18n.changeLanguage(target)
        }
        setSysTick((n) => n + 1)
      })
      .catch(() => setSysTick((n) => n + 1))
    return () => {
      cancelled = true
    }
  }, [i18n])

  // 首帧渲染完成记录
  useEffect(() => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        recordBootstrapStage('first-paint')
      })
    })
  }, [])

  return (
    <div className="flex h-screen w-screen">
      <ErrorBoundary>
        <Suspense fallback={<PageFallback />}>
          <Routes>
            <Route path="/" element={<ErrorBoundary><Home /></ErrorBoundary>} />
            <Route path="/*" element={<ErrorBoundary><AdminLayout /></ErrorBoundary>} />
          </Routes>
        </Suspense>
      </ErrorBoundary>
      <UpdateChecker />
      <DesktopNotifyToast />
      <PerfMonitor />
    </div>
  )
}

/**
 * 路由级 fallback：chunk 到位前用页面形状的骨架占住版面。
 *
 * 刻意不 import ui/Skeleton——App.tsx 静态引入的任何组件都会把 ui/ 提进
 * main index chunk（体积门禁红线）。这里手写的 shimmer 行与 Skeleton 共用
 * index.css 的 .shimmer 底座，观感一致且零额外依赖。
 */
function PageFallback() {
  const { t } = useTranslation()
  return (
    <div role="status" aria-label={t('common.loading')} className="w-full space-y-3">
      <div aria-hidden="true" className="space-y-3">
        <div className="shimmer h-5 w-40 rounded-sm bg-[var(--bg-hover)]" />
        <div className="shimmer h-3 w-full rounded-sm bg-[var(--bg-hover)]" />
        <div className="shimmer h-3 w-full rounded-sm bg-[var(--bg-hover)]" />
        <div className="shimmer h-3 w-2/3 rounded-sm bg-[var(--bg-hover)]" />
      </div>
    </div>
  )
}

function NotFound() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
      <Icon name="search" size={26} />
      <p className="text-[length:var(--app-text-lg)] font-semibold">{t('common.pageNotFound')}</p>
      <p className="max-w-sm text-[length:var(--app-text-sm)] leading-[var(--app-lh-sm)] text-[var(--text-secondary)]">
        {t('common.pageNotFoundDesc')}
      </p>
      <button
        onClick={() => navigate('/')}
        className="btn-ghost mt-2 h-8 px-3 text-[length:var(--app-text-md)]"
      >
        {t('nav.workspace')}
      </button>
    </div>
  )
}

function AdminLayout() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  // 兜底值必须是空串而不是一个写死的版本号：getVersion() 只在 Tauri 里可用，
  // 失败时（浏览器 / IPC 未就绪）显示 "v2.0.0" 这种过期数字是在报错版本。
  const [appVersion, setAppVersion] = useState('')

  useEffect(() => {
    let cancelled = false
    getVersion()
      .then((version) => {
        if (!cancelled) setAppVersion(version)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  return (
    <div className="flex h-screen w-screen">
      <aside className="w-56 bg-[var(--bg-secondary)] border-r border-[var(--border)] flex flex-col">
        <div className="shrink-0 border-b border-[var(--border)] p-4">
          <button onClick={() => navigate('/')} className="text-left">
            <h1 className="text-lg font-bold text-[var(--accent)]">DevEco Switch</h1>
            <p className="mt-1 text-[length:var(--app-text-xs)] text-[var(--text-secondary)]">
              {t('nav.workspace')} →
            </p>
          </button>
        </div>
        {/* min-h-0 不可省：flex 子项的 min-height 默认 auto，15 个导航项会把 nav
            撑到内容高度，footer 直接被顶出视口（657px 高的窗口下版本号 / 通知 /
            语言 / 主题四个控件全都点不到）。滚动只能发生在列表内部。 */}
        <nav className="min-h-0 flex-1 space-y-1 overflow-y-auto p-2">
          {navItems.map((item) => (
            <NavLink
              key={item.path}
              to={item.path}
              end={item.path === '/' || item.path === '/skills'}
              draggable={false}
              className={({ isActive }) =>
                `flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-colors ${
                  isActive
                    ? 'tab-active shadow-[0_0_0_3px_var(--accent-soft)]'
                    : 'tab-inactive'
                }`
              }
            >
              <Icon name={item.icon} size={18} />
              <span>{t(item.labelKey)}</span>
            </NavLink>
          ))}
        </nav>
        <div className="flex shrink-0 items-center justify-between border-t border-[var(--border)] p-4">
          {appVersion && (
            <span className="text-[length:var(--app-text-xs)] text-[var(--text-secondary)] tnum">
              v{appVersion}
            </span>
          )}
          {/* ml-auto 而不是靠 justify-between：版本号缺席时按钮组仍贴右，不产生位移 */}
          <div className="ml-auto flex items-center gap-1">
            <NotificationBell />
            <LangToggle />
            <ThemeToggle />
          </div>
        </div>
      </aside>

      <main className="flex-1 overflow-y-auto p-6">
        <Suspense fallback={<PageFallback />}>
          <Routes>
            <Route path="/lan" element={<LanPage />} />
            <Route path="/providers" element={<ProvidersPage />} />
            <Route path="/versions" element={<VersionsPage />} />
            <Route path="/config" element={<ConfigPage />} />
            <Route path="/limits" element={<LimitsPage />} />
            <Route path="/cost" element={<CostPage />} />
            <Route path="/proxy" element={<ProxyPage />} />
            <Route path="/mcp" element={<McpPage />} />
            <Route path="/skills" element={<SkillsPage />} />
            <Route path="/team-sharing" element={<TeamSharingPage />} />
            <Route path="/reproduction-bundles" element={<ReproductionBundlesPage />} />
            <Route path="/knowledge" element={<KnowledgePage />} />
            <Route path="/api-knowledge" element={<ApiKnowledgePage />} />
            <Route path="/health" element={<HealthPage />} />
            <Route path="/ohpm" element={<OhpmPage />} />
            <Route path="*" element={<NotFound />} />
          </Routes>
        </Suspense>
      </main>
    </div>
  )
}

function ThemeToggle() {
  const { t } = useTranslation()
  const { resolved, toggle } = useThemeStore()
  // 图标指向点击后会切换到的目标主题：深色下是太阳，浅色下是月亮
  const label = resolved === 'dark' ? t('common.themeLight') : t('common.themeDark')
  return (
    <button
      onClick={toggle}
      title={label}
      aria-label={label}
      // 不写 transition-colors：index.css 的无层控件过渡规则已给所有 button
      // 统一挂上 120ms 过渡，该 utility 在这类元素上优先级更低，写了也不生效
      className="rounded p-1.5 hover:bg-[var(--bg-card)]"
    >
      <Icon name={resolved === 'dark' ? 'sun' : 'moon'} size={16} />
    </button>
  )
}
