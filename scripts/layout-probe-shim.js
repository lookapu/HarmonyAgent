/**
 * 布局探针的 shim 入口（由 scripts/layout-shim-build.mjs 打成 IIFE 后注入页面）。
 *
 * 单独一个文件的原因：vite 的 lib 模式需要一个真实入口文件，
 * 而 layout-fixtures.mjs 还要被 --self-test 直接 import 做形状断言，
 * 把两者放一个文件会让「入口」这种东西混进被测数据里。
 */
import { mockIPC } from '@tauri-apps/api/mocks'
import { FIXTURES, PLUGIN_DEFAULTS, FROZEN_NOW } from './layout-fixtures.mjs'

// ===== 冻结时钟 =====
// 会话行上有三处逻辑依赖「当前时间」：时间分组、归档建议徽章（7/30 天两档）、
// 相对时间显示。不冻结的话，这些会随真实日期漂移，基线某天自己就变了，
// 而没人知道是哪天、为什么。**必须在任何应用代码跑之前钉死。**
//
// 只改 Date，不碰 performance / setTimeout：
// React 调度器用 performance.now()，定时器用 setTimeout，
// 一并改会让渲染时序变得不可预测，而时序恰恰决定「2.5s 之后 DOM 稳不稳」。
//
// 无参 `new Date()` 与 `Date.now()` 才被替换；`new Date(x)` / `Date.parse` 照常，
// 否则 moment 之类的库会直接崩。
const RealDate = Date
const FROZEN_MS = FROZEN_NOW * 1000
function ShimDate(...args) {
  if (args.length === 0) return new RealDate(FROZEN_MS)
  return new RealDate(...args)
}
ShimDate.prototype = RealDate.prototype
ShimDate.now = () => FROZEN_MS
ShimDate.parse = RealDate.parse
ShimDate.UTC = RealDate.UTC
globalThis.Date = ShimDate

const called = []
const missing = []
window.__probe = { called, missing }

// 预置 localStorage，把「只在新装机器上出现一次」的浮层关掉：
//   - 首启引导弹窗（会锁 body 滚动，改变视口可用高度）
//   - 语言 / 主题的首次探测
// **必须关掉**：不关的话量到的是「首启那一瞬」的布局，
// 而绝大多数用户看到的是常态布局——两者不是同一个东西。
// localStorage 与页面同源，shim 在导航前注入所以来得及写。
//
// 键名逐字取自 `src/constants.ts` 的 STORAGE_KEYS，**不要凭印象拼**：
// 上一版写的是 `lang` / `deveco-switch:theme`（真实键是 `deveco-switch-lang` /
// `deveco-switch-theme`），两个都写不进去——而「写不进去」没有任何外部信号，
// 页面照样渲染、测量照样稳定，只有��「这个 preset 到底生效没有」时才会发现。
// 下面第三道门禁（check-density 的 fixture 对账）就是为这类「写了但没生效」存在的。
try {
  localStorage.setItem('deveco-switch:first-run-seen', '1')
  localStorage.setItem('deveco-switch-lang', 'zh')
  localStorage.setItem('deveco-switch-theme', 'dark')
  // 「发现新版本」浮层**没有** localStorage 键：UpdateChecker 的 dismissed 是
  // 组件内 useState，刷新即忘。唯一的关法是让 fixture 报告无更新，
  // 见 layout-fixtures.mjs 的 PLUGIN_DEFAULTS['plugin:updater|check']。
} catch { /* 无痕模式下写不进去，退回默认态 */ }

mockIPC(async (cmd) => {
  called.push(cmd)
  if (Object.prototype.hasOwnProperty.call(FIXTURES, cmd)) return FIXTURES[cmd]
  if (Object.prototype.hasOwnProperty.call(PLUGIN_DEFAULTS, cmd)) return PLUGIN_DEFAULTS[cmd]
  // 没 fixture 的命令记下来并返回 null，不抛错。
  // 抛错的代价是整页进 ErrorBoundary，后面所有测量都失去意义；
  // 记下来的代价是这一条命令不贡献数据——由探针在结尾显式报「缺哪些」。
  missing.push(cmd)
  return null
}, { shouldMockEvents: true })

// 官方 mock 不提供 window 元信息访问器
window.__TAURI_INTERNALS__.metadata = {
  currentWindow: { label: 'main' },
  currentWebview: { label: 'main' },
}
