/**
 * 中文拼音匹配工具：
 * - 全拼：用 pinyin-pro 把中文字符转为无声调拼音（如 "首页" → "shouye"）
 * - 首字母：提取每个汉字拼音首字母（如 "首页" → "sy"）
 * 两者均用于 @ 引用候选搜索。对非中文字符原样保留并小写。
 */
import { pinyin } from 'pinyin-pro'

/** 转全拼小写：中文字符 → 拼音，其他字符原样小写保留 */
export function toPinyinFull(s: string): string {
  // pinyin-pro 的 pinyin() 默认按词识别、带空格；type: 'array' 逐字取拼音再拼接
  const arr = pinyin(s, { toneType: 'none', type: 'array', nonZh: 'consecutive' }) as string[]
  return arr.join('').toLowerCase().replace(/\s+/g, '')
}

/** 转拼音首字母小写：中文字符取首字母，其他字符原样小写 */
export function toPinyinInitials(s: string): string {
  const arr = pinyin(s, { pattern: 'first', toneType: 'none', type: 'array', nonZh: 'consecutive' }) as string[]
  return arr.join('').toLowerCase().replace(/\s+/g, '')
}

/**
 * 判断文本是否匹配查询：原文包含、全拼包含、或首字母串包含查询词。
 * 查询词本身是中文时直接走原文匹配。
 */
export function pinyinMatch(text: string, query: string): boolean {
  if (!query) return true
  const q = query.toLowerCase()
  if (text.toLowerCase().includes(q)) return true
  if (toPinyinFull(text).includes(q)) return true
  if (toPinyinInitials(text).includes(q)) return true
  return false
}
