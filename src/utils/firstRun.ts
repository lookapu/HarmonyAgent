/**
 * 首启引导的「看过没有」标记
 *
 * 为什么单独一个文件：Home 需要在**首帧**就决定要不要弹引导（放进 useState 的
 * 初始化函数里），而引导组件本体要走 lazy() 加载。若把 hasSeenFirstRun 写在
 * Onboarding.tsx 里并从 Home 静态 import，那条 import 会把整个引导组件
 * （Modal + health/provider 两个 API 模块）一起拖进 Home 的首屏 chunk，
 * lazy 就白写了。
 *
 * 代价是这个文件只被两处消费，但它换来的正是「引导不进首屏」这一条。
 */

import { STORAGE_KEYS } from '../constants'
import { getItem, setItem } from './storage'

/** 引导是否已经看过（或被用户显式跳过）。 */
export function hasSeenFirstRun(): boolean {
  return getItem(STORAGE_KEYS.FIRST_RUN_SEEN) === '1'
}

/** 标记为已看过：此后不再自动弹出。 */
export function markFirstRunSeen(): void {
  setItem(STORAGE_KEYS.FIRST_RUN_SEEN, '1')
}

/** 清除标记：设置页「重新查看引导」用。 */
export function clearFirstRunSeen(): void {
  try {
    localStorage.removeItem(STORAGE_KEYS.FIRST_RUN_SEEN)
  } catch {
    /* 隐私模式下写不进去，忽略 */
  }
}
