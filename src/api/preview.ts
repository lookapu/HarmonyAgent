import { invokeWithError } from './invoke'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

/** 打开（或导航聚焦）Web 预览窗口：独立窗口加载 http/https 地址 */
export const openPreviewWindow = (url: string) => invokeWithError<void>('open_preview_window', { url })

/** 设备预览会话（后端驱动 SDK 的 Previewer 引擎） */
export interface PreviewSession {
  port: number
  sid: string
  module: string
  device: string
  width: number
  height: number
}

/** 一帧渲染结果：JPEG 的 base64（后端已按帧格式切好） */
export interface PreviewFrame {
  jpeg: string
  bytes: number
}

export interface PreviewStartArgs {
  /** 工程根目录 */
  project: string
  module?: string
  page?: string
  device?: string
  width?: number
  height?: number
}

/** 启动设备预览：内部会先做预览构建，再起引擎；失败信息可直接展示给用户 */
export const previewStart = (args: PreviewStartArgs) =>
  invokeWithError<PreviewSession>('preview_start', {
    module: null,
    page: null,
    device: null,
    width: null,
    height: null,
    ...args,
  })

/** 停止设备预览，释放引擎进程 */
export const previewStop = () => invokeWithError<void>('preview_stop')

/** 订阅渲染帧 */
export const onPreviewFrame = (cb: (frame: PreviewFrame) => void): Promise<UnlistenFn> =>
  listen<PreviewFrame>('preview-frame', (e) => cb(e.payload))

/** 订阅预览错误（引擎侧或取帧中断） */
export const onPreviewError = (cb: (message: string) => void): Promise<UnlistenFn> =>
  listen<string>('preview-error', (e) => cb(e.payload))
