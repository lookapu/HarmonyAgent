import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import Onboarding from './Onboarding'
import { toolsHealth } from '../api/health'
import { listProviders } from '../api/provider'
import { hasSeenFirstRun, markFirstRunSeen, clearFirstRunSeen } from '../utils/firstRun'
import { track } from '../utils/telemetry'
import { STORAGE_KEYS } from '../constants'

const t = (k: string) => (k.includes('.') ? k.split('.').pop()! : k)
const changeLanguage = vi.fn()
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t, i18n: { changeLanguage } }),
  initReactI18next: { type: '3rdParty', init: () => {} },
}))
vi.mock('../api/health', () => ({ toolsHealth: vi.fn() }))
vi.mock('../api/provider', () => ({ listProviders: vi.fn() }))
vi.mock('../utils/telemetry', () => ({ track: vi.fn() }))

const ok = (found: boolean, name: string) => ({
  name,
  found,
  detail: found ? '/opt/huawei/hdc' : '',
  suggestion: found ? null : '请安装 command-line-tools',
})

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
  vi.mocked(toolsHealth).mockResolvedValue([ok(true, 'hvigorw'), ok(true, 'hdc'), ok(true, 'ohpm')])
  vi.mocked(listProviders).mockResolvedValue([])
})

const renderOnboarding = (props: Partial<React.ComponentProps<typeof Onboarding>> = {}) =>
  render(<Onboarding open onClose={() => {}} {...props} />)

describe('首启标记', () => {
  it('初始未看过；标记后看过；清除后回到未看', () => {
    expect(hasSeenFirstRun()).toBe(false)
    markFirstRunSeen()
    expect(hasSeenFirstRun()).toBe(true)
    expect(localStorage.getItem(STORAGE_KEYS.FIRST_RUN_SEEN)).toBe('1')
    clearFirstRunSeen()
    expect(hasSeenFirstRun()).toBe(false)
  })
})

describe('Onboarding', () => {
  it('第 1 步是环境体检，并列出每项结论', async () => {
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    expect(screen.getByText('hvigorw')).toBeInTheDocument()
    expect(screen.getByText('hdc')).toBeInTheDocument()
  })

  // 「体检跑不起来」与「全都缺失」是两件事，不能都显示成红色告警
  it('体检失败时给出重试入口，而不是把结论报成「未找到」', async () => {
    vi.mocked(toolsHealth).mockRejectedValue(new Error('ipc down'))
    renderOnboarding()
    // 用 `envFailed: `（带冒号空格）而不是 `envFailed`——后者也会匹配
    // `envFailedHint`，getByText 撞上两个元素。
    await waitFor(() => expect(screen.getByText(/^envFailed: /)).toBeInTheDocument())
    expect(screen.getByText('envFailedHint')).toBeInTheDocument()
    expect(screen.queryByText('missing')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: /retry/ })).toBeInTheDocument()
  })

  it('有缺失项时把 suggestion 一并摆出来', async () => {
    vi.mocked(toolsHealth).mockResolvedValue([ok(false, 'hdc')])
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('missing')).toBeInTheDocument())
    expect(screen.getByText('请安装 command-line-tools')).toBeInTheDocument()
  })

  it('全部就绪时不显示「去配置路径」——没有缺失就没必要引导去改路径', async () => {
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('envAllGood')).toBeInTheDocument())
    expect(screen.queryByText('envGoSettings')).not.toBeInTheDocument()
  })

  it('可以翻到第 2/3 步，也能退回来', async () => {
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    expect(screen.getByText('project')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    expect(screen.getByText('provider')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'back' }))
    expect(screen.getByText('project')).toBeInTheDocument()
  })

  it('已有项目时第 2 步直接显示为已完成', async () => {
    renderOnboarding({ hasProjects: true })
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    expect(screen.getByText('projectDone')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'projectAdd' })).not.toBeInTheDocument()
  })

  it('无 Provider 时第 3 步给出跳转入口', async () => {
    const onGoProviders = vi.fn()
    renderOnboarding({ onGoProviders })
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    await waitFor(() => expect(screen.getByText('providerNone')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'providerGo' }))
    expect(onGoProviders).toHaveBeenCalled()
  })

  it('已有 Provider 时第 3 步显示为已完成', async () => {
    vi.mocked(listProviders).mockResolvedValue([{ id: '1' }, { id: '2' }] as never)
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    await waitFor(() => expect(screen.getByText('providerDone')).toBeInTheDocument())
  })

  // 引导的价值在于把人送到能干活的那一步；「添加工程」必须真的打开对话框
  it('第 2 步的「添加工程」回调到调用方', async () => {
    const onAddProject = vi.fn()
    renderOnboarding({ onAddProject })
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    fireEvent.click(screen.getByRole('button', { name: 'projectAdd' }))
    expect(onAddProject).toHaveBeenCalled()
  })

  // 标记必须落在「跳过/完成」上：只 setState 关闭的话，重开应用又会弹一遍
  it('「跳过」会写标记，且调 onClose', async () => {
    const onClose = vi.fn()
    renderOnboarding({ onClose })
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'skip' }))
    expect(hasSeenFirstRun()).toBe(true)
    expect(onClose).toHaveBeenCalled()
  })

  it('「开始使用」同样写标记', async () => {
    const onClose = vi.fn()
    renderOnboarding({ onClose })
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    fireEvent.click(screen.getByRole('button', { name: 'done' }))
    expect(hasSeenFirstRun()).toBe(true)
    expect(onClose).toHaveBeenCalled()
  })

  it('Provider 查询失败不影响环境体检的结论（两路互不依赖）', async () => {
    vi.mocked(listProviders).mockRejectedValue(new Error('db locked'))
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('hvigorw')).toBeInTheDocument())
    expect(screen.getByText('envAllGood')).toBeInTheDocument()
  })
})

/**
 * 埋点分支的判定：**走完 = 完成，中途离开 = 跳过**。
 *
 * 这条区分是本组用例的全部价值所在——把两者都记成「完成」的话，
 * 面板上的「引导完成率」会接近 100%，而它本来是唯一能量化引导效果的指标。
 */
describe('Onboarding 埋点', () => {
  it('第 1 步点「跳过」记为 skipped，并带上停留的步骤', async () => {
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'skip' }))
    expect(track).toHaveBeenCalledWith('onboarding_skipped', { step: 'env' })
    expect(track).not.toHaveBeenCalledWith('onboarding_finished', expect.anything())
  })

  it('走完第 3 步点「开始使用」记为 finished', async () => {
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    fireEvent.click(screen.getByRole('button', { name: 'next' }))
    fireEvent.click(screen.getByRole('button', { name: 'done' }))
    expect(track).toHaveBeenCalledWith('onboarding_finished', { step: 'provider' })
  })

  it('到达去重：来回翻不重复计，只按首次到达的顺序记', async () => {
    renderOnboarding()
    await waitFor(() => expect(screen.getByText('env')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'next' })) // → project
    fireEvent.click(screen.getByRole('button', { name: 'back' })) // → env（已到达过）
    fireEvent.click(screen.getByRole('button', { name: 'next' })) // → project（已到达过）
    fireEvent.click(screen.getByRole('button', { name: 'next' })) // → provider（首次）

    const steps = vi.mocked(track).mock.calls
      .filter((c) => c[0] === 'onboarding_step_viewed')
      .map((c) => (c[1] as { step: string }).step)
    expect(steps).toEqual(['env', 'project', 'provider'])
  })
})
