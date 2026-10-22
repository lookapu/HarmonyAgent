import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import ConfigPage from './ConfigPage'
import { readConfig, getConfigPath, getDataScale, clearContentData } from '../api/config'
import { useThemeStore } from '../stores/themeStore'
import { useFontScaleStore } from '../stores/fontScaleStore'

// t 原样返回 key：断言不依赖完整 i18n 初始化
// initReactI18next 仍需给出——ConfigPage 从 ../i18n 取 LANG_STORAGE_KEY，
// 那会执行 i18next 初始化并要求这个插件。给个空插件即可，不影响 useTranslation 的桩。
const t = (k: string) => k
const changeLanguage = vi.fn()
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t, i18n: { changeLanguage } }),
  initReactI18next: { type: '3rdParty', init: () => {} },
}))
vi.mock('../api/config', () => ({
  readConfig: vi.fn(),
  writeConfig: vi.fn(),
  getConfigPath: vi.fn(),
  clearContentData: vi.fn(),
  runMaintenance: vi.fn(),
  exportBackup: vi.fn(),
  getDataScale: vi.fn(),
}))
vi.mock('../api/desktop', () => ({ detectSystemLocale: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(getDataScale).mockResolvedValue({
    conversations: 3,
    messages: 40,
    request_logs: 7,
    task_runs: 2,
    project_memories: 1,
  })
  vi.mocked(readConfig).mockResolvedValue({ provider: {} })
  vi.mocked(getConfigPath).mockResolvedValue('/home/u/.config/deveco/deveco.jsonc')
  useThemeStore.setState({ theme: 'dark', resolved: 'dark' })
  useFontScaleStore.setState({ scale: 'md' })
  document.documentElement.style.removeProperty('--ui-font-scale')
  localStorage.clear()
})

// 设置页里「重新查看引导」用 useNavigate 跳回工作区，故必须包一层 Router
const renderPage = () =>
  render(
    <MemoryRouter>
      <ConfigPage />
    </MemoryRouter>,
  )

describe('ConfigPage 三段结构', () => {
  it('常规区直接给出主题与语言的控件，不要求用户碰 JSON', () => {
    renderPage()
    expect(screen.getByRole('group', { name: 'config.theme' })).toBeInTheDocument()
    expect(screen.getByRole('group', { name: 'config.lang' })).toBeInTheDocument()
  })

  it('切主题走 themeStore，而不是写进 config.json', async () => {
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: 'config.themeLight' }))
    await waitFor(() => expect(useThemeStore.getState().theme).toBe('light'))
    expect(readConfig).not.toHaveBeenCalled()
  })

  // 高级区是逃生舱，不该在常规路径上抢一次磁盘读 + JSON.parse
  it('默认折叠，且折叠时完全不读 config 文件', () => {
    renderPage()
    expect(screen.queryByText('config.advancedLabel')).not.toBeInTheDocument()
    expect(readConfig).not.toHaveBeenCalled()
    expect(getConfigPath).not.toHaveBeenCalled()
  })

  it('展开高级区后才拉取 config', async () => {
    renderPage()
    fireEvent.click(screen.getByRole('button', { expanded: false }))
    await waitFor(() => expect(readConfig).toHaveBeenCalledTimes(1))
    expect(screen.getByText(/deveco\.jsonc$/)).toBeInTheDocument()
  })

  it('展示配置文件路径，出问题时用户知道去哪找', async () => {
    renderPage()
    fireEvent.click(screen.getByRole('button', { expanded: false }))
    await waitFor(() =>
      expect(screen.getByText('/home/u/.config/deveco/deveco.jsonc')).toBeInTheDocument(),
    )
  })

  it('规模拿不到就整块不显示，不给「0」的假象', async () => {
    vi.mocked(getDataScale).mockRejectedValue(new Error('db locked'))
    renderPage()
    await waitFor(() => expect(getDataScale).toHaveBeenCalled())
    expect(screen.queryByText('config.scaleConv')).not.toBeInTheDocument()
  })

  it('规模正常时展示真实计数', async () => {
    renderPage()
    await waitFor(() => expect(screen.getByText('config.scaleConv')).toBeInTheDocument())
    expect(screen.getByText('40')).toBeInTheDocument()
  })

  // 「读不到」和「确实是 0」是两件事：读不到必须给出重试入口
  it('config 加载失败时显示错误并给出真·重试按钮', async () => {
    vi.mocked(readConfig).mockRejectedValue(new Error('EACCES'))
    renderPage()
    fireEvent.click(screen.getByRole('button', { expanded: false }))
    await waitFor(() => expect(screen.getByText(/config.loadError/)).toBeInTheDocument())

    vi.mocked(readConfig).mockResolvedValue({ provider: {} })
    fireEvent.click(screen.getByRole('button', { name: /common.retry/ }))
    await waitFor(() => expect(readConfig).toHaveBeenCalledTimes(2))
  })

  it('清空数据走确认弹层，不直接删', async () => {
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /config.clearAll/ }))
    expect(screen.getByText('config.clearAllConfirm')).toBeInTheDocument()
    expect(clearContentData).not.toHaveBeenCalled()
  })

  it('确认后调用清空接口并展示删除数量', async () => {
    vi.mocked(clearContentData).mockResolvedValue([2, 7])
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: /config.clearAll/ }))
    const buttons = screen.getAllByRole('button', { name: /config.clearAll/ })
    fireEvent.click(buttons[buttons.length - 1])
    await waitFor(() => expect(clearContentData).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(screen.getByText('config.clearAllDone')).toBeInTheDocument())
  })

  it('常规区给出字号档控件', () => {
    renderPage()
    expect(screen.getByRole('group', { name: 'config.fontSize' })).toBeInTheDocument()
  })

  // 字号必须真的改到 DOM，而不是只存了个 localStorage 值
  it('切字号把 --ui-font-scale 写到 <html> 行内样式', () => {
    renderPage()
    fireEvent.click(screen.getByRole('button', { name: 'config.fontLg' }))
    expect(document.documentElement.style.getPropertyValue('--ui-font-scale')).toBe('1.15')
    expect(localStorage.getItem('deveco-switch:font-scale')).toBe('lg')
  })
})
