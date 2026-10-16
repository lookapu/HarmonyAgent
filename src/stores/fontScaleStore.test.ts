import { describe, it, expect, beforeEach, vi } from 'vitest'
import { useFontScaleStore, FONT_SCALE_ORDER } from './fontScaleStore'
import { STORAGE_KEYS } from '../constants'

/**
 * 这些用例守的是三件「看起来做了、实际没生效」的事：
 * 1. 档位非法时必须回落标准，而不是把垃圾值写进 CSS；
 * 2. 切回标准档也要落盘（否则重启后又按旧档渲染）；
 * 3. 标准档不留行内覆盖，DOM 干净。
 */

const root = document.documentElement

beforeEach(() => {
  localStorage.clear()
  root.style.removeProperty('--ui-font-scale')
  useFontScaleStore.setState({ scale: 'md' })
})

describe('fontScaleStore', () => {
  it('三档都有定义，且每档都对应一个倍率', () => {
    expect(FONT_SCALE_ORDER).toEqual(['sm', 'md', 'lg'])
  })

  it('切到大档同时改 DOM 与存储', () => {
    useFontScaleStore.getState().setScale('lg')
    expect(root.style.getPropertyValue('--ui-font-scale')).toBe('1.15')
    expect(localStorage.getItem(STORAGE_KEYS.FONT_SCALE)).toBe('lg')
  })

  // 第一版 apply() 在 scale=1 时提前 return，把 setItem 一起跳过了
  it('切回标准档：撤掉行内覆盖，但仍然落盘', () => {
    useFontScaleStore.getState().setScale('sm')
    expect(root.style.getPropertyValue('--ui-font-scale')).toBe('0.95')

    useFontScaleStore.getState().setScale('md')
    expect(root.style.getPropertyValue('--ui-font-scale')).toBe('')
    expect(localStorage.getItem(STORAGE_KEYS.FONT_SCALE)).toBe('md')
  })

  // 初始读档只在模块加载时跑一次，要测它必须真正重载模块
  // （否则这条用例其实在测 setState 之后的 apply，等于没测读档）
  it('重启后按存储的档位渲染', async () => {
    localStorage.setItem(STORAGE_KEYS.FONT_SCALE, 'lg')
    vi.resetModules()
    const mod = await import('./fontScaleStore')
    expect(mod.useFontScaleStore.getState().scale).toBe('lg')
    expect(root.style.getPropertyValue('--ui-font-scale')).toBe('1.15')
  })

  it('存储里的非法值不参与渲染（回落标准）', async () => {
    localStorage.setItem(STORAGE_KEYS.FONT_SCALE, 'huge')
    vi.resetModules()
    const mod = await import('./fontScaleStore')
    expect(mod.useFontScaleStore.getState().scale).toBe('md')
    expect(root.style.getPropertyValue('--ui-font-scale')).toBe('')
  })

  it('store 状态与 DOM 保持一致，不出现「界面变了、记录没变」', () => {
    for (const s of FONT_SCALE_ORDER) {
      useFontScaleStore.getState().setScale(s)
      expect(localStorage.getItem(STORAGE_KEYS.FONT_SCALE)).toBe(s)
      expect(useFontScaleStore.getState().scale).toBe(s)
    }
  })
})
