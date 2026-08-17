/**
 * 渲染性能追踪工具：使用 performance.mark/measure 记录关键路径耗时，
 * 切换项目/会话后在控制台输出各阶段耗时，并保留历史记录供性能面板查看。
 *
 * 附加能力：
 * - 长任务自动检测：监听 PerformanceObserver 'longtask'，主线程阻塞 >100ms 自动记录
 * - FPS 监控：启用后每 1s 采样一次，连续低 FPS 自动记录卡顿帧
 *
 * 用法：
 *   const trace = startPerfTrace('openProject', { projectId: 'xxx' })
 *   trace.mark('listConvs')
 *   ...
 *   trace.end()
 */

export interface PerfSegment {
  name: string
  /** 本段耗时（毫秒） */
  durationMs: number
}

export type PerfRecordKind = 'trace' | 'longtask' | 'lowfps' | 'bootstrap'

export interface PerfRecord {
  id: string
  label: string
  kind: PerfRecordKind
  meta?: Record<string, string>
  /** 总耗时（毫秒） */
  totalMs: number
  /** 各分段耗时 */
  segments: PerfSegment[]
  /** 记录时间戳（performance.now 相对值，ms） */
  at: number
  /** 墙上时间戳（Date.now()，用于显示绝对时间） */
  wallTime: number
  /** 性能等级：good(绿) <200ms / warn(黄) <500ms / bad(红) >=500ms */
  level: 'good' | 'warn' | 'bad'
}

export interface PerfTrace {
  mark: (name: string) => void
  /** 在浏览器下一次实际绘制后再打标（双 rAF 确保 paint 完成） */
  markAfterPaint: (name: string) => void
  end: () => void
  /** 在 paint 完成后再结束 trace（更准确反映用户感知时间） */
  endAfterPaint: () => void
}

interface TraceEntry {
  id: string
  label: string
  kind: PerfRecordKind
  meta?: Record<string, string>
  start: number
  marks: Array<{ name: string; time: number }>
}

const MAX_HISTORY = 80
const activeTraces = new Map<string, TraceEntry>()
const history: PerfRecord[] = []
const listeners = new Set<(records: PerfRecord[]) => void>()

// ---------- 等级判定 ----------
function computeLevel(totalMs: number, kind: PerfRecordKind = 'trace'): PerfRecord['level'] {
  // longtask/lowfps 阈值更敏感
  if (kind === 'longtask') {
    // 100ms ≈ 掉 6 帧，肉眼已是一次明显「卡一下」；50ms 是浏览器 longtask 自身阈值
    if (totalMs >= 100) return 'bad'
    if (totalMs >= 50) return 'warn'
    return 'good'
  }
  if (kind === 'lowfps') {
    if (totalMs < 30) return 'bad'     // fps < 30 → 红
    if (totalMs < 50) return 'warn'    // fps < 50 → 黄
    return 'good'
  }
  if (totalMs >= 500) return 'bad'
  if (totalMs >= 200) return 'warn'
  return 'good'
}

function emit(record: PerfRecord) {
  history.push(record)
  if (history.length > MAX_HISTORY) history.shift()
  for (const fn of listeners) {
    try { fn([...history]) } catch { /* ignore listener errors */ }
  }
}

/** 订阅性能记录更新；返回取消订阅函数 */
export function subscribePerfRecords(fn: (records: PerfRecord[]) => void): () => void {
  listeners.add(fn)
  fn([...history])
  return () => listeners.delete(fn)
}

/** 获取当前所有历史记录（按时间倒序） */
export function getPerfHistory(): PerfRecord[] {
  return [...history].reverse()
}

/** 清空历史记录 */
export function clearPerfHistory() {
  history.length = 0
  for (const fn of listeners) {
    try { fn([]) } catch { /* ignore */ }
  }
}

// ---------- 核心 API：开始/结束追踪 ----------

/** 开始一段性能追踪；label 用于控制台输出区分 */
export function startPerfTrace(label: string, meta?: Record<string, string>): PerfTrace {
  const id = `${label}-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`
  const start = performance.now()
  const entry: TraceEntry = { id, label, kind: 'trace', meta, start, marks: [] }
  activeTraces.set(id, entry)
  performance.mark(`${id}:start`)
  let finished = false
  const doMark = (name: string) => {
    if (finished) return
    entry.marks.push({ name, time: performance.now() })
    performance.mark(`${id}:${name}`)
  }
  const doEnd = () => {
    if (finished) return
    finished = true
    finishTrace(entry)
  }
  return {
    mark: doMark,
    markAfterPaint(name: string) {
      // 双 rAF：第一个 rAF 在 React commit 之后、paint 之前；第二个 rAF 在 paint 之后
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          doMark(name)
        })
      })
    },
    end: doEnd,
    endAfterPaint() {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          doEnd()
        })
      })
    },
  }
}

function finishTrace(entry: TraceEntry) {
  const end = performance.now()
  const total = end - entry.start
  const segments: PerfSegment[] = []
  let prev = entry.start
  for (const m of entry.marks) {
    segments.push({ name: m.name, durationMs: m.time - prev })
    prev = m.time
  }
  if (entry.marks.length > 0) {
    segments.push({ name: 'rest', durationMs: end - prev })
  }
  const level = computeLevel(total, entry.kind)
  const record: PerfRecord = {
    id: entry.id,
    label: entry.label,
    kind: entry.kind,
    meta: entry.meta,
    totalMs: total,
    segments,
    at: end,
    wallTime: Date.now(),
    level,
  }

  // 控制台彩色输出（非自动监控类始终输出；longtask/lowfps 只在 bad/warn 时输出避免刷屏）
  const shouldLog = entry.kind === 'trace' || entry.kind === 'bootstrap' || level !== 'good'
  if (shouldLog) {
    const parts = segments.map((s) => `${s.name}=${s.durationMs.toFixed(0)}ms`).join(' ')
    const metaStr = entry.meta ? ` ${JSON.stringify(entry.meta)}` : ''
    const style = level === 'bad' ? 'color:#ef4444;font-weight:bold' : level === 'warn' ? 'color:#f59e0b;font-weight:bold' : 'color:#22c55e'
    const prefix = entry.kind === 'longtask' ? '[jank]' : entry.kind === 'lowfps' ? '[fps]' : entry.kind === 'bootstrap' ? '[boot]' : '[perf]'
    console.log(
      `%c${prefix} ${entry.label}${metaStr} total=${total.toFixed(0)}ms ${parts}`,
      style,
    )
  }

  emit(record)
  activeTraces.delete(entry.id)
  try {
    performance.clearMarks(`${entry.id}:start`)
    for (const m of entry.marks) performance.clearMarks(`${entry.id}:${m.name}`)
  } catch { /* ignore */ }
}

// ---------- 自动监控：长任务（主线程阻塞 > 阈值） ----------

let longTaskObserver: PerformanceObserver | null = null

/** 启用长任务监控：主线程阻塞 >100ms 自动记录为 jank */
export function enableLongTaskMonitor() {
  if (longTaskObserver) return
  if (typeof window === 'undefined' || !('PerformanceObserver' in window)) return
  try {
    longTaskObserver = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        const dur = entry.duration
        if (dur < 50) continue // 浏览器 longtask 阈值即 50ms，等于/超过即一次可感知卡顿
        const id = `longtask-${Date.now()}-${Math.random().toString(36).slice(2, 4)}`
        const record: PerfRecord = {
          id,
          label: `longtask ${dur.toFixed(0)}ms`,
          kind: 'longtask',
          totalMs: dur,
          segments: [{ name: 'blocked', durationMs: dur }],
          at: performance.now(),
          wallTime: Date.now(),
          level: computeLevel(dur, 'longtask'),
          meta: { name: entry.name || 'unknown' },
        }
        emit(record)
        if (dur >= 100) {
          console.warn(`%c[jank] 主线程阻塞 ${dur.toFixed(0)}ms（${entry.name || 'unknown'}）`, 'color:#f59e0b')
        }
      }
    })
    longTaskObserver.observe({ entryTypes: ['longtask'] })
  } catch {
    /* longtask 在部分环境可能不支持 */
  }
}

// ---------- 自动监控：长动画帧（LoAF, Long Animation Frame） ----------
// longtask 只能报「某个宏任务 > 50ms」，name 恒为 self，定位不到是哪个脚本/事件；
// LoAF（Chrome 123+）按「帧」衡量，包含渲染/布局耗时，且带 scripts[].sourceURL/invoker，
// 能精准回答「卡在哪一行、哪个事件」——正是那种一瞬即逝的「卡一下」的最佳探测器。

interface LoafScript {
  invoker?: string
  sourceURL?: string
  sourceFunctionName?: string
  duration?: number
}

interface LoafEntry extends PerformanceEntry {
  duration: number
  blockingDuration?: number
  scripts?: LoafScript[]
}

let loafObserver: PerformanceObserver | null = null

function shortScriptName(s: LoafScript | undefined): string {
  if (!s) return 'script'
  const fn = s.sourceFunctionName || s.invoker || ''
  const base = (s.sourceURL || '').split('/').pop() || s.sourceURL || ''
  if (base && fn) return `${base}:${fn}`
  return base || fn || 'script'
}

export function enableLongAnimationFrameMonitor() {
  if (loafObserver) return
  if (typeof window === 'undefined' || !('PerformanceObserver' in window)) return
  try {
    loafObserver = new PerformanceObserver((list) => {
      const entries = list.getEntries() as unknown as LoafEntry[]
      for (const entry of entries) {
        const dur = entry.duration
        if (dur < 50) continue
        const scripts = (entry.scripts || [])
          .filter((s) => (s.duration ?? 0) > 0)
          .sort((a, b) => (b.duration ?? 0) - (a.duration ?? 0))
        const top = scripts[0] // 按脚本耗时降序，首条即卡顿主因
        const record: PerfRecord = {
          id: `laf-${Date.now()}-${Math.random().toString(36).slice(2, 4)}`,
          label: `long-frame ${dur.toFixed(0)}ms`,
          kind: 'longtask',
          totalMs: dur,
          segments: scripts.slice(0, 3).map((s) => ({ name: shortScriptName(s), durationMs: s.duration ?? 0 })),
          at: performance.now(),
          wallTime: Date.now(),
          level: computeLevel(dur, 'longtask'),
          meta: {
            blocking: `${(entry.blockingDuration ?? 0).toFixed(0)}ms`,
            ...(top?.sourceURL ? { src: top.sourceURL } : {}),
            ...(top?.invoker ? { invoker: top.invoker } : {}),
          },
        }
        emit(record)
        if (dur >= 100) {
          console.warn(
            `%c[jank] 长帧 ${dur.toFixed(0)}ms（blocking=${(entry.blockingDuration ?? 0).toFixed(0)}ms）${top ? ' · ' + shortScriptName(top) : ''}`,
            'color:#ef4444',
          )
        }
      }
    })
    loafObserver.observe({ type: 'long-animation-frame' as never })
  } catch {
    /* LoAF 在非 Chromium 或旧内核不可用，静默回退到 longtask */
  }
}

// ---------- 自动监控：FPS ----------

let fpsRafId: number | null = null
let fpsFrames = 0
let fpsLastTime = 0
let fpsLowStreak = 0

/** 启用 FPS 监控：每 1s 采样一次，连续 <30fps 自动记录 */
export function enableFpsMonitor() {
  if (fpsRafId !== null) return
  if (typeof window === 'undefined' || !window.requestAnimationFrame) return
  fpsLastTime = performance.now()
  fpsFrames = 0
  fpsLowStreak = 0
  const tick = (now: number) => {
    fpsFrames++
    const elapsed = now - fpsLastTime
    if (elapsed >= 1000) {
      const fps = Math.round((fpsFrames * 1000) / elapsed)
      if (fps < 45) {
        fpsLowStreak++
        // 连续 2 秒低 FPS 才记录，避免偶发抖动
        if (fpsLowStreak >= 2) {
          const id = `fps-${Date.now()}`
          emit({
            id,
            label: `low FPS: ${fps}`,
            kind: 'lowfps',
            totalMs: fps, // 复用 totalMs 字段存 fps 值
            segments: [{ name: 'fps', durationMs: fps }],
            at: now,
            wallTime: Date.now(),
            level: computeLevel(fps, 'lowfps'),
          })
          fpsLowStreak = 0
        }
      } else {
        fpsLowStreak = 0
      }
      fpsFrames = 0
      fpsLastTime = now
    }
    fpsRafId = requestAnimationFrame(tick)
  }
  fpsRafId = requestAnimationFrame(tick)
}

export function disableFpsMonitor() {
  if (fpsRafId !== null) {
    cancelAnimationFrame(fpsRafId)
    fpsRafId = null
  }
}

// ---------- 启动时间戳：JS 开始执行时打点 ----------
// 通过在模块顶层立即打标，配合 bootstrap trace 可以测量冷启动到交互就绪的耗时

const BOOT_START_KEY = '__devecoBootStart'
declare global {
  interface Window {
    [BOOT_START_KEY]?: number
  }
}
if (typeof window !== 'undefined' && !window[BOOT_START_KEY]) {
  window[BOOT_START_KEY] = performance.now()
}

/** 记录启动阶段耗时：从 JS 开始执行到调用点的时间，配合 stage 标记分阶段 */
export function recordBootstrapStage(stage: string) {
  if (typeof window === 'undefined') return
  const start = window[BOOT_START_KEY] ?? performance.now()
  const now = performance.now()
  const id = `boot-${stage}-${Date.now()}`
  emit({
    id,
    label: `boot.${stage}`,
    kind: 'bootstrap',
    totalMs: now - start,
    segments: [{ name: 'since-start', durationMs: now - start }],
    at: now,
    wallTime: Date.now(),
    level: computeLevel(now - start, 'trace'),
  })
}

/** 等待 React commit + 浏览器 paint 完成（双 rAF） */
export function waitForNextPaint(): Promise<void> {
  return new Promise((resolve) => {
    if (typeof requestAnimationFrame === 'undefined') {
      setTimeout(resolve, 16)
      return
    }
    requestAnimationFrame(() => {
      requestAnimationFrame(() => resolve())
    })
  })
}

/** 测量一段同步操作的耗时（便捷封装：不用手动 start/mark/end） */
export function measureSync<T>(label: string, fn: () => T, meta?: Record<string, string>): T {
  const t = startPerfTrace(label, meta)
  try {
    const result = fn()
    t.end()
    return result
  } catch (e) {
    t.mark('error')
    t.end()
    throw e
  }
}

/** 测量 async 操作耗时 */
export async function measureAsync<T>(label: string, fn: () => Promise<T>, meta?: Record<string, string>): Promise<T> {
  const t = startPerfTrace(label, meta)
  try {
    const result = await fn()
    t.end()
    return result
  } catch (e) {
    t.mark('error')
    t.end()
    throw e
  }
}
