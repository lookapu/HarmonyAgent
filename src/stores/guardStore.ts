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
// 契约：
// - **瞬时状态，不进 localStorage**：它是「当前这一轮」的信号，
//   进程重启后没有意义（护栏状态本身也是进程内的）。
// - **后端只在级别变化时发事件**（stage=0 表示恢复），
//   所以这里不需要节流，也不需要自己去比较前后值。
// - stage 与模型侧文案的分级**同源**（后端 `task_guard::stall_stage` 是单一真源），
//   不会出现「提示条说第 1 级、模型看到的是第 2 级」。
import { create } from 'zustand'

export interface GuardState {
  /** 失速级别：0 = 无 · 1 / 2 / 3 = 阶梯 */
  stage: number
  /** 距离上次实质进展的工具调用数 */
  sinceProgress: number
  /** 停滞期累计进上下文的字节数 */
  stalledBytes: number
  /** 当前失速阈值（连续多少次无进展即触发） */
  threshold: number
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
