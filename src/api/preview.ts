import { invoke } from '@tauri-apps/api/core'

/** 打开（或导航聚焦）Web 预览窗口：独立窗口加载 http/https 地址 */
export const openPreviewWindow = (url: string) => invoke<void>('open_preview_window', { url })
