import { describe, it, expect, vi, beforeEach } from 'vitest'
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react'
import { PreviewPanel } from './panels'
import { onPreviewError, onPreviewFrame, previewStart, previewStop } from '../../api/preview'

// 文案只用于展示：t 原样返回 key，断言不依赖完整 i18n 初始化
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (k: string) => k }) }))
vi.mock('../../api/preview', () => ({
  previewStart: vi.fn(),
  previewStop: vi.fn(),
  onPreviewFrame: vi.fn(),
  onPreviewError: vi.fn(),
  openPreviewWindow: vi.fn(),
}))

type FrameCb = (frame: { jpeg: string; bytes: number }) => void
let frameCbs: FrameCb[] = []
let errorCbs: Array<(msg: string) => void> = []
let unsubscribed = 0

const panel = (project?: string) =>
  render(<PreviewPanel url="" setUrl={() => {}} src="" onOpen={() => {}} project={project} />)

beforeEach(() => {
  frameCbs = []
  errorCbs = []
  unsubscribed = 0
  vi.mocked(onPreviewFrame).mockImplementation((cb: FrameCb) => {
    frameCbs.push(cb)
    return Promise.resolve(() => {
      unsubscribed++
    })
  })
  vi.mocked(onPreviewError).mockImplementation((cb: (msg: string) => void) => {
    errorCbs.push(cb)
    return Promise.resolve(() => {
      unsubscribed++
    })
  })
  vi.mocked(previewStart).mockResolvedValue({
    port: 29999,
    sid: 'abc',
    module: 'entry',
    device: 'phone',
    width: 1080,
    height: 2340,
  })
  vi.mocked(previewStop).mockResolvedValue(undefined)
})

describe('PreviewPanel 设备预览', () => {
  it('没有工程时不能启动预览', () => {
    panel(undefined)
    expect(screen.getByRole('button', { name: 'home.previewDeviceStart' })).toBeDisabled()
  })

  it('点击启动会用工程路径调用后端，并把帧画出来', async () => {
    panel('H:/proj/demo')
    fireEvent.click(screen.getByRole('button', { name: 'home.previewDeviceStart' }))
    await waitFor(() => expect(previewStart).toHaveBeenCalledWith({ project: 'H:/proj/demo' }))
    await waitFor(() => expect(frameCbs).toHaveLength(1))

    // 引擎推来一帧后应显示为图片，而不是继续停在空态
    act(() => frameCbs[0]({ jpeg: 'QUJD', bytes: 3 }))
    const img = await screen.findByRole('img', { name: 'home.previewDeviceTitle' })
    expect(img).toHaveAttribute('src', 'data:image/jpeg;base64,QUJD')
    expect(screen.queryByText('home.previewDeviceEmpty')).not.toBeInTheDocument()
  })

  it('启动后按钮切换为停止，点击停止会释放引擎', async () => {
    panel('H:/proj/demo')
    fireEvent.click(screen.getByRole('button', { name: 'home.previewDeviceStart' }))
    const stop = await screen.findByRole('button', { name: 'home.previewDeviceStop' })
    fireEvent.click(stop)
    await waitFor(() => expect(previewStop).toHaveBeenCalled())
  })

  it('启动失败时把后端原因显示出来，不静默', async () => {
    vi.mocked(previewStart).mockRejectedValueOnce(new Error('未找到 Previewer 引擎'))
    panel('H:/proj/demo')
    fireEvent.click(screen.getByRole('button', { name: 'home.previewDeviceStart' }))
    expect(await screen.findByText(/home\.previewDeviceFailed：未找到 Previewer 引擎/)).toBeInTheDocument()
    // 失败后不应进入 live：仍是可重试的启动按钮
    expect(screen.getByRole('button', { name: 'home.previewDeviceStart' })).toBeEnabled()
  })

  it('取帧中断的错误也会显示出来', async () => {
    panel('H:/proj/demo')
    await waitFor(() => expect(errorCbs).toHaveLength(1))
    act(() => errorCbs[0]('预览引擎在握手阶段关闭了连接'))
    expect(await screen.findByText(/预览引擎在握手阶段关闭了连接/)).toBeInTheDocument()
  })

  it('卸载时解绑订阅并停掉引擎', async () => {
    const view = panel('H:/proj/demo')
    await waitFor(() => expect(frameCbs).toHaveLength(1))
    view.unmount()
    await waitFor(() => expect(unsubscribed).toBe(2))
    expect(previewStop).toHaveBeenCalled()
  })
})
