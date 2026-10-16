/**
 * 中文拼音匹配工具（**懒加载**）
 * - 全拼：中文字符 → 无声调拼音（如 "首页" → "shouye"）
 * - 首字母：取每个汉字拼音首字母（如 "首页" → "sy"）
 * 两者均用于 @ 引用候选搜索与命令面板过滤。对非中文字符原样保留并小写。
 *
 * ## 为什么是懒加载
 *
 * 2026-10-01 量化 Home chunk 的产物归因时发现：`pinyin-pro` 的 4 本词典
 * （dict1~4 + surname）合计 **105.8KB，占 Home chunk 的 30.7%**——
 * 是仅次于 Home.tsx 自身的第二大项。
 *
 * 但拼音只在两处用到，且**都不是首屏必需**：
 * 1. 聊天输入框打 `@` 后的文件候选排序（用户要先打字）；
 * 2. 命令面板输入查询词后的过滤（要先打开面板）。
 *
 * 也就是说这 105.8KB 是在首屏**必然用不到**、只在交互后才用到的数据。
 *
 * ## 为什么签名仍然是同步的
 *
 * 排序与过滤都在渲染路径上（一个 map + sort），改成 async 会把整条链路
 * 变成异步重排。所以本模块对外**保持同步签名**，用「未就绪时降级」换取：
 * - `toPinyin*` 未就绪返回 `''`（表示"没有拼音可匹配"），调用方自然落到
 *   子序列匹配等后续分支；
 * - 就绪后 `onPinyinReady` 回调触发调用方重算，**用户不需要重输一次**。
 *
 * 降级不等于「只工作一半」：调用方**必须**订阅 `onPinyinReady` 并在就绪后重算，
 * 否则就是"搜不到拼音且不自愈"。两个消费点（Home / CommandPalette）都订阅了。
 */
import type { PinyinOptions, PinyinResult } from './pinyinTypes'

/** 已解析的 pinyin 函数；null 表示词典尚未加载 */
type PinyinFn = (s: string, o?: PinyinOptions) => PinyinResult

let pinyinFn: PinyinFn | null = null
let inflight: Promise<void> | null = null
const listeners = new Set<() => void>()

/**
 * 触发词典加载。重复调用只发起一次网络/磁盘请求。
 * 失败时**不缓存失败状态**：清掉 inflight，下次调用会重试——
 * 一个永远失败的懒加载比「每次都重试」更糟，因为用户会以为功能不存在。
 */
export function preloadPinyin(): Promise<void> {
  if (pinyinFn) return Promise.resolve()
  if (inflight) return inflight
  inflight = import('pinyin-pro')
    .then((m) => {
      pinyinFn = m.pinyin as unknown as PinyinFn
      inflight = null
      // 就绪通知放在微任务之后，让订阅方有机会在本轮渲染中已订阅好
      queueMicrotask(() => {
        for (const l of listeners) {
          try {
            l()
          } catch {
            /* 单个订阅者出错不应影响其他订阅者与加载状态 */
          }
        }
      })
    })
    .catch(() => {
      inflight = null
    })
  return inflight
}

/** 词典是否已就绪；false 时拼音匹配整体不可用（调用方应重算一次） */
export function isPinyinReady(): boolean {
  return pinyinFn !== null
}

/** 订阅「词典就绪」。返回取消订阅函数。加载完成后回调只触发一次。 */
export function onPinyinReady(cb: () => void): () => void {
  if (pinyinFn) {
    // 已就绪：立刻通知一次，让调用方无条件重算（省掉「是否已就绪」的分支）
    cb()
    return () => {}
  }
  listeners.add(cb)
  return () => {
    listeners.delete(cb)
  }
}

/** 未就绪时返回空串：表示"没有拼音可匹配"，调用方落到后续匹配分支 */
function convert(s: string, pattern: 'full' | 'first'): string {
  if (!pinyinFn) {
    // 词典加载需要一次磁盘读取；只在真的有中文时才值得触发
    if (/[\u4e00-\u9fa5]/.test(s)) void preloadPinyin()
    return ''
  }
  const arr = pinyinFn(s, {
    ...(pattern === 'first' ? { pattern: 'first' } : {}),
    toneType: 'none',
    type: 'array',
    nonZh: 'consecutive',
  }) as string[]
  return arr.join('').toLowerCase().replace(/\s+/g, '')
}

/** 转全拼小写：中文字符 → 拼音，其他字符原样小写保留 */
export function toPinyinFull(s: string): string {
  return convert(s, 'full')
}

/** 转拼音首字母小写：中文字符取首字母，其他字符原样小写保留 */
export function toPinyinInitials(s: string): string {
  return convert(s, 'first')
}

/**
 * 判断文本是否匹配查询：原文包含、全拼包含、或首字母串包含查询词。
 * 词典未就绪时只剩「原文包含」——调用方需订阅 onPinyinReady 重算。
 */
export function pinyinMatch(text: string, query: string): boolean {
  if (!query) return true
  const q = query.toLowerCase()
  if (text.toLowerCase().includes(q)) return true
  if (toPinyinFull(text).includes(q)) return true
  if (toPinyinInitials(text).includes(q)) return true
  return false
}
