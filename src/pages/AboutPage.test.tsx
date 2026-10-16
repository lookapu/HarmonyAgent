import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import AboutPage, { buildNotice } from './AboutPage'
import manifest from '../generated/thirdParty.json'
import { getVersion, getTauriVersion } from '@tauri-apps/api/app'
import { save as dialogSave } from '@tauri-apps/plugin-dialog'
import { writeTextFile } from '@tauri-apps/plugin-fs'

/**
 * About 页要回答的是「这东西能不能商用分发」，所以测试盯三件事：
 *
 * 1. **非宽松许可必须默认可见**，不能藏在「展开全部」里——
 *    折叠宽松项是为了可读性，折叠 copyleft 则是把需要人做决定的东西藏起来。
 * 2. **版本读不到时不许编**。显示一个错的版本号比显示「—」更糟：
 *    用户会拿它去对 bug 报告，而错误的版本号会让那份报告被丢掉。
 * 3. **NOTICE 导出必须真的写文件**，而且要包含 copyleft 项——
 *    Apache-2.0 / MPL-2.0 的义务是「随分发件提供」，只在页面上展示不满足。
 */

// 插值版 i18n：NOTICE 数量这类要断言的键必须真的把参数渲染出来，
// 否则断言只能验证「键名出现了」——那等于什么都没验证。
// 拼接形式保留键名，便于用 /key/ 正则定位，又能让插值值被断言到。
const t = (k: string, o?: Record<string, unknown>) => {
  const key = k.includes('.') ? k.split('.').pop()! : k
  if (o?.count !== undefined) return `${key}(${o.count})`
  if (o?.version !== undefined) return `${key}:${o.version}`
  return key
}
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t, i18n: { changeLanguage: vi.fn() } }),
  initReactI18next: { type: '3rdParty', init: () => {} },
}))
vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(),
  getTauriVersion: vi.fn(),
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: vi.fn() }))
vi.mock('@tauri-apps/plugin-fs', () => ({ writeTextFile: vi.fn() }))

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(getVersion).mockResolvedValue('2.3.0')
  vi.mocked(getTauriVersion).mockResolvedValue('2.11.5')
  vi.mocked(dialogSave).mockResolvedValue('C:/tmp/NOTICE.txt')
  vi.mocked(writeTextFile).mockResolvedValue(undefined)
})

describe('AboutPage 许可信息', () => {
  it('四档计数都来自真实清单，不写死', async () => {
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    // 四个计数卡：permissive / notice / copyleft / unknown
    for (const tier of ['permissive', 'notice', 'copyleft', 'unknown']) {
      expect(screen.getByText(tier)).toBeInTheDocument()
    }
    const totals = manifest.totals as Record<string, number>
    expect(screen.getByText(String(totals.tier_copyleft))).toBeInTheDocument()
  })

  it('copyleft 与无法判定的项默认展开说明，不需要点「展开全部」', async () => {
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    // 折叠列表此时不该存在，但「需要关注的许可」必须在
    expect(screen.getByText('needsAttention')).toBeInTheDocument()
    // 豁免卡里同时出现组件名与 MPL 说明
    expect(screen.getByText(/cssparser-macros/)).toBeInTheDocument()
    expect(screen.getAllByText(/MPL-2\.0/).length).toBeGreaterThan(0)
    // 展开按钮还在，说明宽松清单确实是折叠的
    expect(screen.getByRole('button', { name: /showList/ })).toBeInTheDocument()
  })

  it('宽松项的完整清单默认折叠，展开后可见', async () => {
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    expect(screen.queryByText('Apache-2.0')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /showList/ }))
    expect(screen.getAllByText('Apache-2.0').length).toBeGreaterThan(0)
  })

  it('清单里的组件数与生成产物一致（页面没在用另一份数据）', async () => {
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    const totals = manifest.totals as Record<string, number>
    // 插值版 mock 会把 count 渲染出来
    expect(screen.getByText(`thirdPartySource(${totals.all})`)).toBeInTheDocument()
  })
})

describe('AboutPage 版本', () => {
  it('版本取不到时显示占位符而不是编一个版本号', async () => {
    vi.mocked(getVersion).mockRejectedValue(new Error('ipc down'))
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText(/versionError/)).toBeInTheDocument())
    // 关键：界面上不能出现任何看起来像版本号的字符串
    expect(screen.queryByText('2.3.0')).not.toBeInTheDocument()
  })

  it('Tauri 运行时版本与 app 版本分开显示', async () => {
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    // 单一插值：`Tauri 2.11.5` 落在一个文本节点里，读屏与断言都不用绕
    expect(screen.getByText(/2\.11\.5/)).toBeInTheDocument()
  })
})

describe('AboutPage 导出 NOTICE', () => {
  it('真的写文件，且内容含 copyleft 项与总数', async () => {
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'exportNotice' }))
    await waitFor(() => expect(writeTextFile).toHaveBeenCalled())
    const body = vi.mocked(writeTextFile).mock.calls[0][1] as string
    expect(body).toContain('MPL-2.0')
    expect(body).toContain(String((manifest.totals as Record<string, number>).all))
    expect(body).toContain('NOTICE')
    expect(body).toContain('Apache-2.0')
  })

  it('用户取消保存时不算失败，也不写文件', async () => {
    vi.mocked(dialogSave).mockResolvedValue(null)
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'exportNotice' }))
    await waitFor(() => expect(dialogSave).toHaveBeenCalled())
    expect(writeTextFile).not.toHaveBeenCalled()
    expect(screen.queryByText('noticeExported')).not.toBeInTheDocument()
  })

  it('写文件失败要报出来，不能静默', async () => {
    vi.mocked(writeTextFile).mockRejectedValue(new Error('EACCES'))
    render(<AboutPage />)
    await waitFor(() => expect(screen.getByText('2.3.0')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'exportNotice' }))
    await waitFor(() => expect(screen.getByText('noticeFailed')).toBeInTheDocument())
  })
})

describe('buildNotice', () => {
  it('按档位分段，copyleft 在最前', () => {
    const groups = [
      { ecosystem: 'npm', license: 'MIT', tier: 'permissive' as const, nonSpdx: false, count: 2, names: ['react 19', 'zustand 5'] },
      { ecosystem: 'cargo', license: 'MPL-2.0', tier: 'copyleft' as const, nonSpdx: false, count: 1, names: ['cssparser 0.36'] },
    ]
    const text = buildNotice(groups, { all: 3, tier_permissive: 2, tier_notice: 0, tier_copyleft: 1, tier_unknown: 0 })
    expect(text.indexOf('[COPYLEFT]')).toBeLessThan(text.indexOf('[PERMISSIVE]'))
    expect(text).toContain('cssparser 0.36')
    expect(text).toContain('react 19')
  })

  it('非 SPDX 标识会被标出来（不能让它看起来像已判定）', () => {
    const groups = [
      { ecosystem: 'npm', license: 'SEE LICENSE IN LICENSE.txt', tier: 'unknown' as const, nonSpdx: true, count: 1, names: ['@ohos/hvigor 6'] },
    ]
    const text = buildNotice(groups, { all: 1, tier_permissive: 0, tier_notice: 0, tier_copyleft: 0, tier_unknown: 1 })
    expect(text).toContain('no SPDX identifier')
    expect(text).toContain('@ohos/hvigor 6')
  })
})
