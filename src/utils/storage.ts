/**
 * 统一前端持久化封装。
 *
 * 目的：收敛散落在各组件 / store 中的 `localStorage` 直接调用与重复的 try/catch 容错样板。
 *
 * 约定：
 * - 所有读写经此封装；localStorage 不可用（无痕模式 / 配额满）时静默失败，不抛错。
 * - 提供 JSON 序列化辅助（getJSON / setJSON），解析失败返回 fallback，避免调用方各自 try/catch。
 * - 键名集中在 src/constants.ts 的 STORAGE_KEYS，调用方传入完整键，避免硬编码与重复定义。
 *
 * 兼容性说明：键的字符串值刻意保持与历史一致，以兼容用户已写入的 localStorage 数据；
 * 不在此处强制统一前缀（历史键存在 `deveco-switch:*` 与 `deveco-*` 两种形态，统一前缀需
 * 配套数据迁移，超出本次封装范围）。
 */

/** 读取字符串；不可用 / 缺失返回 null。 */
export function getItem(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

/** 写入字符串；不可用 / 配额满静默失败。 */
export function setItem(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  } catch {
    /* 配额满 / 无痕模式 → 静默失败 */
  }
}

/** 删除键；不可用静默失败。 */
export function removeItem(key: string): void {
  try {
    localStorage.removeItem(key)
  } catch {
    /* ignore */
  }
}

/** 读取并 JSON 解析；缺失 / 解析失败返回 fallback。 */
export function getJSON<T>(key: string, fallback: T): T {
  const raw = getItem(key)
  if (raw == null) return fallback
  try {
    return JSON.parse(raw) as T
  } catch {
    return fallback
  }
}

/** JSON 序列化后写入；不可用 / 序列化失败静默失败。 */
export function setJSON(key: string, value: unknown): void {
  try {
    setItem(key, JSON.stringify(value))
  } catch {
    /* ignore */
  }
}
