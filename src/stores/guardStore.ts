// ============================================================
// Agent 失速提示状态（per conversation，**不持久化**）
// ============================================================
//
// 为什么需要它：
// 失速护栏此前只把阶梯文案**注入模型的提示词**，用户**完全看不见**——
// 界面上一片安静，后台却在按 T-8 实测的量烧上下文
// （9 字提问 → 209,825 输入 token，其中工具总耗时只占 2.2%）。
//
// ⇒ 把同一个数字同时给用户看：**连续 N 次无实质进展 · 停滞期累计 X KB 进上下文**。
// 用户据此可以主动叫停，而不是等账单说话。
//
// ⚠️ 第三十三批补上「**它到底在干什么**」：
// 「连续 24 次」是个没有信息量的数字——用户分不清
// 「反复读同一个文件」（真打转，该叫停）与「读了 24 个不同文件」（是探索）。
// ⇒ 带上**失速画像**（重复最多的动作 + 主导工具），与模型侧**同一份数据**。
//
// 契约：
// - **瞬时状态，不进 localStorage**：它是「当前这一轮」的信号，
//   进程重启后没有意义（护栏状态本身也是进程内的）。
// - **后端只在级别变化时发事件**（stage=0 表示恢复），
//   所以这里不需要节流，也不需要自己去比较前后值。
// - stage 与模型侧文案的分级**同源**（后端 `task_guard::stall_stage` 是单一真源），
//   不会出现「提示条说第 1 级、模型看到的是第 2 级」。
import { create } from 'zustand'

/** 一个「重复动作」：同一工具 + 同一标识参数（同一文件 / 同一搜索词 / 同一条命令）。 */
export interface GuardActionStat {
  tool: string
  /** 展示目标（路径 / 搜索词 / 命令…）；工具没有标识参数时为 null */
  target: string | null
  /** 重复次数 */
  count: number
}

/** 主导工具：停滞窗口内被调用最多的工具（**跨目标聚合**）。 */
export interface GuardToolStat {
  tool: string
  count: number
}

/**
 * 失速画像：**它到底在干什么**。
 *
 * 「连续 24 次没有实质进展」是个没有信息量的数字——用户分不清
 * 「反复读同一个文件」（真打转）和「读了 24 个不同文件」（是探索）。
 * 这份画像把两者分开，而且**与模型侧看到的完全同源**。
 */
export interface GuardProfile {
  /** 重复次数 ≥ 2 的动作（次数降序） */
  repeats: GuardActionStat[]
  /** 窗口内调用最多的工具；窗口为空时为 null */
  topTool: GuardToolStat | null
}

export interface GuardState {
  /** 失速级别：0 = 无 · 1 / 2 / 3 = 阶梯 */
  stage: number
  /** 距离上次实质进展的工具调用数 */
  sinceProgress: number
  /** 停滞期累计进上下文的字节数 */
  stalledBytes: number
  /** 当前失速阈值（连续多少次无进展即触发） */
  threshold: number
  /** 它到底在干什么 */
  profile: GuardProfile
  /** 事件到达时间（毫秒时间戳） */
  at: number
}

interface GuardStore {
  /** conversationId -> 失速状态（只有失速中的会话才有条目） */
  guards: Record<string, GuardState>
  /** 记录一次失速状态 */
  set: (convId: string, state: GuardState) => void
  /** 清空某会话的失速状态（恢复时调用） */
  clear: (convId: string) => void
}

export const useGuardStore = create<GuardStore>((set, get) => ({
  guards: {},
  set: (convId, state) => {
    // ⚠️ 同一个 convId 重复 set 时要换新对象引用，否则订阅该条的组件不会重渲染
    set({ guards: { ...get().guards, [convId]: { ...state } } })
  },
  clear: (convId) => {
    if (!get().guards[convId]) return
    const next = { ...get().guards }
    delete next[convId]
    set({ guards: next })
  },
}))

/** 把字节数格式化成「X KB / X MB」——用户读的是量级，不是字节数。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 KB'
  const kb = bytes / 1024
  if (kb < 1024) return `${kb < 10 ? kb.toFixed(1) : Math.round(kb)} KB`
  return `${(kb / 1024).toFixed(1)} MB`
}

/**
 * 展示目标过长时**中间截断**（头尾都留）。
 *
 * 为什么不是只留尾部：路径的尾部更有信息量（`src/a/b/c.ets`），
 * 但**搜索词/命令**的头部更有信息量（`npm run build -- --watch`）。
 * ⇒ 两头都留，是唯一不偏科的折中。
 */
export function truncateTarget(text: string, max = 48): string {
  if (text.length <= max) return text
  const head = Math.ceil((max - 1) / 2)
  const tail = max - 1 - head
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`
}
