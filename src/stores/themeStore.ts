import { create } from 'zustand'

type Theme = 'dark' | 'light'

interface ThemeStore {
  theme: Theme
  toggle: () => void
  setTheme: (theme: Theme) => void
}

const getInitialTheme = (): Theme => {
  const stored = localStorage.getItem('deveco-switch-theme')
  if (stored === 'light' || stored === 'dark') return stored
  return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'
}

export const useThemeStore = create<ThemeStore>((set) => ({
  theme: getInitialTheme(),
  toggle: () =>
    set((state) => {
      const next = state.theme === 'dark' ? 'light' : 'dark'
      localStorage.setItem('deveco-switch-theme', next)
      document.documentElement.setAttribute('data-theme', next)
      return { theme: next }
    }),
  setTheme: (theme) => {
    localStorage.setItem('deveco-switch-theme', theme)
    document.documentElement.setAttribute('data-theme', theme)
    set({ theme })
  },
}))

// Apply theme on load
document.documentElement.setAttribute('data-theme', getInitialTheme())
