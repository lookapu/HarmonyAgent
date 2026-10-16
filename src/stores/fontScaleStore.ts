/**
 * 界面字号档位
 *
 * 存在的意义：index.css 的 7 档字号阶梯在 2026-10-01 之前是**装饰品**——
 * 全库 1238 处写的是 `text-[Npx]` 这样的字面量，改 --app-text-xs 没有任何界面反应。
 * 那批字面量已全部改为 token 形式之后，这个开关才第一次真的有东西可缩。
 *
 * 档位取值 0.95 / 1 / 1.15 不是随手取的：三档下每一档乘出来的实际像素
 * 都落在**本仓历史上已出现过的字号**附近（0.95 → micro 档 8.55px，库里有过 8.5px；
 * 1.15 → base 档 16.1px，库里有过 18px 与 15px）。也就是说这两档不是没跑过的构图，
 * 而是回到曾经验证过的密度。行高同步缩放，否则字号变大而行高不变会立刻挤成一团。
 *
 * 存的是档位名（sm/md/lg）而不是倍率：倍率是实现细节，
 * 日后调参不该让用户已存的值变成非法值。
 */
import { create } from 'zustand'
import { STORAGE_KEYS } from '../constants'
import { getItem, setItem } from '../utils/storage'

/** 字号档：小 / 标准 / 大 */
export type FontScale = 'sm' | 'md' | 'lg'

/** 档位 → --ui-font-scale 倍率 */
const SCALE_VALUE: Record<FontScale, string> = {
  sm: '0.95',
  md: '1',
  lg: '1.15',
}

export const FONT_SCALE_ORDER: readonly FontScale[] = ['sm', 'md', 'lg']

const isFontScale = (v: string | null): v is FontScale =>
  v === 'sm' || v === 'md' || v === 'lg'

/** 从 localStorage 读档；缺失或非法一律落回 md（= 保持原样的倍数） */
const getInitialScale = (): FontScale => {
  const stored = getItem(STORAGE_KEYS.FONT_SCALE)
  return isFontScale(stored) ? stored : 'md'
}

/**
 * 写到 <html> 的行内样式
 *
 * 为什么必须行内：行内样式优先级高于 :root 的 @layer theme 声明，
 * 所以这里一写就全库生效，不需要改任何 className。
 * 同时**不在 scale=1 时写**：默认档保持 :root 里的 --ui-font-scale: 1 生效，
 * DOM 上不留下无意义的行内属性，DevTools 里也看不出异常覆盖。
 */
const apply = (scale: FontScale): void => {
  // 先落盘再改 DOM：顺序反了会出现「切到标准档不写盘」，
  // 于是重启后又按上次的旧档（如 sm）渲染，而用户看到的是标准档。
  setItem(STORAGE_KEYS.FONT_SCALE, scale)
  const value = SCALE_VALUE[scale]
  if (value === '1') {
    document.documentElement.style.removeProperty('--ui-font-scale')
    return
  }
  document.documentElement.style.setProperty('--ui-font-scale', value)
}

interface FontScaleStore {
  scale: FontScale
  setScale: (scale: FontScale) => void
}

export const useFontScaleStore = create<FontScaleStore>((set) => ({
  scale: getInitialScale(),
  setScale: (scale) => {
    apply(scale)
    set({ scale })
  },
}))

// 初始化：模块加载即应用，保证首帧就是用户上次选的字号
// （与 themeStore 同一模式；放进 effect 会闪一帧默认字号再跳）
const initialScale = getInitialScale()
apply(initialScale)
useFontScaleStore.setState({ scale: initialScale })
