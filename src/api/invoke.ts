import { invoke as tauriInvoke } from '@tauri-apps/api/core'

/**
 * 统一 Tauri IPC 调用封装层。
 *
 * 目的：收敛散落在 30+ api 文件中的 `invoke` 直接调用与各自的错误处理逻辑。
 *
 * 行为约定：
 * - 成功返回 T，失败**重抛原始错误**，因此调用方的 catch 行为（错误类型 / 文案）保持不变，
 *   可安全替换裸 invoke，无需改动上层 UI 的错误处理。
 * - 统一记录调用日志（命令名 + 脱敏参数），便于排障；后续如需集中接入性能埋点、
 *   可重试策略或错误映射，只需修改本文件，无需改动各调用点。
 */

/** 对参数做轻量脱敏，避免把 token / 路径等敏感字段打进日志。 */
function redact(args: Record<string, unknown> | undefined): Record<string, unknown> | undefined {
  if (!args) return undefined
  const sensitive = new Set(['token', 'apiKey', 'api_key', 'password', 'secret', 'key'])
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(args)) {
    out[k] = sensitive.has(k.toLowerCase()) ? '<redacted>' : v
  }
  return out
}

export async function invokeWithError<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args)
  } catch (e) {
    console.error(`[invoke] ${cmd} failed`, redact(args), e)
    throw e
  }
}

/**
 * 失败静默变体：invoke 抛错时返回 fallback，不抛出。
 * 用于「失败不影响主流程」的场景（如草稿 / 偏好读取），替代各处的 try/catch 样板。
 */
export async function invokeSafe<T>(
  cmd: string,
  args: Record<string, unknown> | undefined,
  fallback: T,
): Promise<T> {
  try {
    return await tauriInvoke<T>(cmd, args)
  } catch (e) {
    console.warn(`[invoke] ${cmd} failed, using fallback`, redact(args), e)
    return fallback
  }
}
