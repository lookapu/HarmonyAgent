import { create } from 'zustand'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { getItem, setItem } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

/** 主题：auto 跟随系统；dark/light 显式指定 */
export type Theme = 'dark' | 'light' | 'auto'
type ResolvedTheme = 'dark' | 'light'

interface ThemeStore {
  theme: Theme
  /** 当前解析后的主题（auto 时跟系统） */
  resolved: ResolvedTheme
  toggle: () => void
  setTheme: (theme: Theme) => void
  /** 解析 auto：优先用 matches，否则用 last-known */
  resolve: () => ResolvedTheme
}

/** 同步窗口标题栏主题（Windows 生效；非 Tauri 环境或调用失败时忽略） */
const syncWindowTheme = (theme: ResolvedTheme) => {
  try {
    void getCurrentWindow()
      .setTheme(theme)
      .catch(() => {})
  } catch {
    // 浏览器调试环境无 Tauri 注入，忽略
  }
}

/** 根据 mode 解析实际生效主题；不读 store，避免循环 */
const resolveTheme = (mode: Theme, last: ResolvedTheme): ResolvedTheme => {
  if (mode === 'dark' || mode === 'light') return mode
  return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : last
}

const getInitialMode = (): Theme => {
  const stored = getItem(STORAGE_KEYS.THEME)
  if (stored === 'dark' || stored === 'light' || stored === 'auto') return stored
  return 'auto'
}

const apply = (mode: Theme) => {
  const last = (getItem(STORAGE_KEYS.THEME_LAST) as ResolvedTheme) || 'dark'
  const resolved = resolveTheme(mode, last)
  document.documentElement.setAttribute('data-theme', resolved)
  setItem(STORAGE_KEYS.THEME, mode)
  setItem(STORAGE_KEYS.THEME_LAST, resolved)
  syncWindowTheme(resolved)
  return resolved
}

export const useThemeStore = create<ThemeStore>((set, get) => ({
  theme: getInitialMode(),
  resolved: 'dark',
  toggle: () => {
    const cur = get().resolved
    const next: ResolvedTheme = cur === 'dark' ? 'light' : 'dark'
    document.documentElement.setAttribute('data-theme', next)
    setItem(STORAGE_KEYS.THEME, next)
    setItem(STORAGE_KEYS.THEME_LAST, next)
    syncWindowTheme(next)
    set({ theme: next, resolved: next })
  },
  setTheme: (theme) => {
    const resolved = apply(theme)
    set({ theme, resolved })
  },
  resolve: () => {
    const mode = get().theme
    return resolveTheme(mode, get().resolved)
  },
}))

// 初始化：应用主题 + 注册系统切换监听（auto 模式下跟随）
const initialMode = getInitialMode()
const initialResolved = apply(initialMode)
useThemeStore.setState({ theme: initialMode, resolved: initialResolved })

// 监听系统主题变化（仅 auto 模式生效）
const mql = window.matchMedia('(prefers-color-scheme: light)')
mql.addEventListener('change', () => {
  const state = useThemeStore.getState()
  if (state.theme === 'auto') {
    const resolved = apply('auto')
    useThemeStore.setState({ resolved })
  }
})
