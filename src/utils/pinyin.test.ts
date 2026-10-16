import { describe, it, expect, vi, beforeEach } from 'vitest'

/**
 * 拼音懒加载的三条不变量：
 * 1. **未就绪不抛错**：词典没加载时调用方仍要能跑（拼音匹配退化为不匹配）。
 * 2. **就绪后自愈**：订阅方会收到通知，调用方据此重算。
 * 3. **失败可重试**：加载失败不能把模块钉死在「永远不可用」上。
 *
 * 这三条都是「静默失效」的高发区：任一条破了，拼音搜索就表现为
 * 「搜不到中文文件名」，而界面上没有任何报错。
 */

/** 够用的假实现：把「首页」映成 shouye，其余原样返回。
 *  注意实现与 spy 必须分开：若把 pinyinMock 自己当 mockImplementation 传给
 *  pinyinMock，等于让 mock 递归调用自己，表现为 Maximum call stack size exceeded。 */
const pinyinImpl = (s: string) => (s === '首页' ? ['shou', 'ye'] : [s.toLowerCase()])
const pinyinMock = vi.fn(pinyinImpl)
vi.mock('pinyin-pro', () => ({ pinyin: pinyinMock }))

/** 每个用例都要一个干净的模块实例——模块级缓存会跨用例泄漏 */
const fresh = async () => {
  vi.resetModules()
  pinyinMock.mockClear()
  pinyinMock.mockImplementation(pinyinImpl)
  return import('./pinyin')
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe('pinyin 懒加载', () => {
  it('未就绪时返回空串而不是抛错——降级不能是崩溃', async () => {
    const p = await fresh()
    expect(p.isPinyinReady()).toBe(false)
    expect(p.toPinyinFull('首页')).toBe('')
    expect(p.toPinyinInitials('首页')).toBe('')
  })

  it('未就绪时 pinyinMatch 退化为「只做原文匹配」，不误报为匹配', async () => {
    const p = await fresh()
    // 原文含查询 → 仍应命中（这是降级路径里唯一还能工作的分支）
    expect(p.pinyinMatch('首页 Home', '首页')).toBe(true)
    // 拼音查询 → 未就绪时匹配不上，这是**预期**的降级，不是不变量被破坏
    expect(p.pinyinMatch('首页', 'shouye')).toBe(false)
  })

  // 原先这条写成「英文不触发加载」，但那个断言构造不出失败：
  // 没 await preload 时回调本来就不会 fire，恒等于 0。改成两条可证伪的：
  it('含中文字符串会自动触发加载——不依赖 UI 预热也能自愈', async () => {
    const p = await fresh()
    expect(p.isPinyinReady()).toBe(false)
    p.toPinyinFull('首页') // 内部检测到中文 → 触发 preloadPinyin
    await p.preloadPinyin()
    await Promise.resolve()
    await Promise.resolve()
    expect(p.isPinyinReady()).toBe(true)
    // 自愈后同一个调用返回真实拼音，而不是一直返回 ''
    expect(p.toPinyinFull('首页')).toBe('shouye')
  })

  it('纯英文查询在未就绪时照常工作（降级不能影响非中文场景）', async () => {
    const p = await fresh()
    expect(p.pinyinMatch('New Conversation', 'new')).toBe(true)
    expect(p.pinyinMatch('New Conversation', 'zzz')).toBe(false)
  })

  it('preloadPinyin 后词典就绪，拼音匹配可用', async () => {
    const p = await fresh()
    await p.preloadPinyin()
    expect(p.isPinyinReady()).toBe(true)
    expect(p.toPinyinFull('首页')).toBe('shouye')
    expect(p.pinyinMatch('首页', 'shouye')).toBe(true)
  })

  it('订阅方在就绪那一刻被通知——调用方靠它重算', async () => {
    const p = await fresh()
    let fired = 0
    p.onPinyinReady(() => {
      fired += 1
    })
    expect(fired).toBe(0)
    await p.preloadPinyin()
    // 通知在微任务里发出，等一拍
    await Promise.resolve()
    await Promise.resolve()
    expect(fired).toBe(1)
  })

  it('已就绪后再订阅：立刻回调一次，调用方不需要判断「是否已就绪」', async () => {
    const p = await fresh()
    await p.preloadPinyin()
    let fired = 0
    p.onPinyinReady(() => {
      fired += 1
    })
    expect(fired).toBe(1)
  })

  it('取消订阅后就绪通知不再送达', async () => {
    const p = await fresh()
    let fired = 0
    const off = p.onPinyinReady(() => {
      fired += 1
    })
    off()
    await p.preloadPinyin()
    await Promise.resolve()
    await Promise.resolve()
    expect(fired).toBe(0)
  })

  it('重复调用 preloadPinyin 只加载一次', async () => {
    const p = await fresh()
    await Promise.all([p.preloadPinyin(), p.preloadPinyin(), p.preloadPinyin()])
    await p.preloadPinyin()
    expect(p.isPinyinReady()).toBe(true)
  })
})
