import { Routes, Route, NavLink, useNavigate } from 'react-router-dom'
import { lazy, Suspense, useEffect, useLayoutEffect, useState } from 'react'
import { getVersion } from '@tauri-apps/api/app'
import { useTranslation } from 'react-i18next'
import Icon, { type IconName } from './icons/Icon'
import UpdateChecker from './components/UpdateChecker'
import DesktopNotifyToast from './components/DesktopNotifyToast'
import PerfMonitor from './components/PerfMonitor'
import NotificationBell from './components/NotificationBell'
import { ErrorBoundary } from './components/ErrorBoundary'
import { recordBootstrapStage } from './utils/perfTrace'
import { useThemeStore } from './stores/themeStore'
import { detectSystemLocale } from './api/desktop'
import { LANG_STORAGE_KEY } from './i18n'
import { detectGpu, getTierClass } from './utils/gpuDetect'
import { getItem, setItem } from './utils/storage'

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
  { path: '/team-sharing', labelKey: 'nav.teamSharing', icon: 'skill' },
  { path: '/reproduction-bundles', labelKey: 'nav.reproductionBundles', icon: 'archive' },
  { path: '/knowledge', labelKey: 'nav.knowledge', icon: 'skill' },
  { path: '/api-knowledge', labelKey: 'nav.apiKnowledge', icon: 'package' },
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

function PageFallback() {
  return (
    <div className="flex h-full w-full items-center justify-center bg-[var(--bg-primary)] text-[var(--text-secondary)]">
      <span className="h-5 w-5 animate-spin rounded-full border-2 border-[var(--border)] border-t-[var(--accent)]" />
    </div>
  )
}

function AdminLayout() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [appVersion, setAppVersion] = useState('2.0.0')

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
        <div className="p-4 border-b border-[var(--border)]">
          <button onClick={() => navigate('/')} className="text-left">
            <h1 className="text-lg font-bold text-[var(--accent)]">DevEco Switch</h1>
            <p className="text-xs text-[var(--text-secondary)] mt-1">
              {t('nav.workspace')} →
            </p>
          </button>
        </div>
        <nav className="flex-1 p-2 space-y-1">
          {navItems.map((item) => (
            <NavLink
              key={item.path}
              to={item.path}
              end={item.path === '/' || item.path === '/skills'}
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
        <div className="p-4 border-t border-[var(--border)] flex items-center justify-between">
          <span className="text-xs text-[var(--text-secondary)]">v{appVersion}</span>
          <div className="flex items-center gap-1">
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
          </Routes>
        </Suspense>
      </main>
    </div>
  )
}

function LangToggle() {
  const { i18n, t } = useTranslation()
  const saved = getItem(LANG_STORAGE_KEY)
  const mode: 'auto' | 'zh' | 'en' = saved === 'auto' ? 'auto' : i18n.language === 'en' ? 'en' : 'zh'

  const cycle = async () => {
    // 中 → EN → 自动 → 中
    const nextMode = mode === 'zh' ? 'en' : mode === 'en' ? 'auto' : 'zh'
    if (nextMode === 'auto') {
      setItem(LANG_STORAGE_KEY, 'auto')
      try {
        const info = await detectSystemLocale()
        i18n.changeLanguage(info.is_zh ? 'zh' : 'en')
      } catch {
        i18n.changeLanguage('zh')
      }
    } else {
      setItem(LANG_STORAGE_KEY, nextMode)
      i18n.changeLanguage(nextMode)
    }
  }

  const label = mode === 'auto' ? 'AUTO' : mode === 'zh' ? 'EN' : '中'
  const title =
    mode === 'auto'
      ? t('common.langSystem')
      : mode === 'zh'
        ? 'Switch to English'
        : '切换到中文'

  return (
    <button
      onClick={cycle}
      className="px-1.5 py-0.5 rounded text-xs font-mono hover:bg-[var(--bg-card)] transition-colors text-[var(--text-secondary)]"
      title={title}
    >
      {label}
    </button>
  )
}

function ThemeToggle() {
  const { resolved, toggle } = useThemeStore()
  return (
    <button
      onClick={toggle}
      className="p-1.5 rounded hover:bg-[var(--bg-card)] transition-colors"
      title={resolved === 'dark' ? 'Light mode' : 'Dark mode'}
    >
      {resolved === 'dark' ? (
        <svg width="16" height="16" viewBox="0 -960 960 960" fill="currentColor">
          <path d="M480-360q50 0 85-35t35-85q0-50-35-85t-85-35q-50 0-85 35t-35 85q0 50 35 85t85 35Zm0 80q-83 0-141.5-58.5T280-480q0-83 58.5-141.5T480-680q83 0 141.5 58.5T680-480q0 83-58.5 141.5T480-280ZM200-440H40v-80h160v80Zm720 0H760v-80h160v80ZM440-760v-160h80v160h-80Zm0 720v-160h80v160h-80ZM256-650l-101-97 57-59 96 100-52 56Zm492 496-97-101 53-55 101 97-57 59Zm-98-550 97-101 59 57-100 96-56-52ZM154-212l101-97 55 53-97 101-59-57Zm326-268Z"/>
        </svg>
      ) : (
        <svg width="16" height="16" viewBox="0 -960 960 960" fill="currentColor">
          <path d="M480-120q-150 0-255-105T120-480q0-150 105-255t255-105q14 0 27.5 1t26.5 3q-41 29-65.5 75.5T444-660q0 90 63 153t153 63q55 0 101-24.5t75-65.5q2 13 3 26.5t1 27.5q0 150-105 255T480-120Zm0-80q88 0 158-48.5T740-375q-20 5-40 8t-40 3q-123 0-209.5-86.5T364-660q0-20 3-40t8-40q-78 32-126.5 102T200-480q0 116 82 198t198 82Zm-10-270Z"/>
        </svg>
      )}
    </button>
  )
}
