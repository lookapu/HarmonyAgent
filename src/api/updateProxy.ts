import { check } from '@tauri-apps/plugin-updater'
import { beginUpdateProxy, endUpdateProxy, getSystemProxy } from './update'

/**
 * 检查/下载前应用系统代理（双保险）：
 * 1. 显式传 proxy 给 updater（check({ proxy })，检查+下载+安装全程生效）
 * 2. 临时注入环境变量兜底（updater 内部 reqwest 默认读环境变量）
 * 结束后恢复环境变量快照。
 */
export async function withProxy<T>(fn: (proxy?: string) => Promise<T>): Promise<T> {
  const proxy = await getSystemProxy().catch(() => null)
  const saved = await beginUpdateProxy().catch(() => [])
  try {
    return await fn(proxy ?? undefined)
  } finally {
    await endUpdateProxy(saved).catch(() => {})
  }
}

/** 带代理的更新检查（显式传入系统代理地址） */
export const checkWithProxy = (proxy?: string) => check(proxy ? { proxy } : undefined)
