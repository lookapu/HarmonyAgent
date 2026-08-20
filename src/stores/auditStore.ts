// ============================================================
// 审计日志 store（敏感操作流水）：localStorage 持久化，滚动 200 条
// ============================================================
//
// 设计目标：
// - 凡是"不可撤销 / 影响数据"的操作都打一条记录（删除会话/项目、清空队列、停止任务、回滚等）
// - 纯前端方案：避免动后端 Rust 端（[149] 已有 runtime_log，但用途偏运行时错误诊断）
// - localStorage 持久化：用户清缓存/卸载前可见；上限 200 条 FIFO
// - 不与 notifications 重复：通知是"我刚做完"的瞬时提示；审计是"历史可查"
import { create } from 'zustand'
import { getJSON, setJSON } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

export type AuditCategory =
  | 'conversation.delete'   // 删除会话
  | 'conversation.archive'  // 归档/取消归档
  | 'conversation.pin'      // 置顶
  | 'conversation.fork'     // fork
  | 'conversation.import'   // 导入预览
  | 'message.delete'        // 删除消息
  | 'project.remove'        // 删除项目
  | 'project.trust'         // 信任项目
  | 'queue.clear'           // 清空排队
  | 'task.stop'             // 停止任务
  | 'task.rollback'         // 任务回滚
  | 'task.timeline'         // 会话时间旅行（回到历史快照点）
  | 'rules.update'          // 更新规则
  | 'config.update'         // 更新配置

export interface AuditEntry {
  /** 唯一 id（时间戳 + 计数） */
  id: string
  /** 操作时间（unix ms） */
  ts: number
  /** 操作分类 */
  category: AuditCategory
  /** 简短可读名（i18n key 已解析） */
  label: string
  /** 详情（自由文本：被删除对象/受影响条数/原因等） */
  detail?: string
  /** 关联项目 id */
  projectId?: string
  /** 关联会话 id */
  conversationId?: string
}

const MAX_ENTRIES = 200

const loadFromStorage = (): AuditEntry[] => {
  const arr = getJSON<unknown>(STORAGE_KEYS.AUDIT_LOG, null)
  return Array.isArray(arr) ? (arr as AuditEntry[]).slice(-MAX_ENTRIES) : []
}

const saveToStorage = (list: AuditEntry[]) => {
  // 限长 + 限制字段大小（防 XSS 把 audit 撑爆）
  const trimmed = list.slice(-MAX_ENTRIES).map((e) => ({
    ...e,
    label: e.label.slice(0, 100),
    detail: e.detail ? e.detail.slice(0, 500) : undefined,
  }))
  setJSON(STORAGE_KEYS.AUDIT_LOG, trimmed)
}

let _seq = 0
const newId = () => `a${Date.now().toString(36)}${(++_seq).toString(36)}`

interface AuditStore {
  entries: AuditEntry[]
  /** 记录一条审计（自动写 localStorage） */
  log: (entry: Omit<AuditEntry, 'id' | 'ts'>) => void
  /** 清空所有 */
  clear: () => void
  /** 按分类过滤 */
  filter: (category: AuditCategory | 'all') => AuditEntry[]
}

export const useAuditStore = create<AuditStore>((set, get) => ({
  entries: loadFromStorage(),
  log: (entry) => {
    const full: AuditEntry = { id: newId(), ts: Date.now(), ...entry }
    const next = [...get().entries, full].slice(-MAX_ENTRIES)
    set({ entries: next })
    saveToStorage(next)
  },
  clear: () => {
    set({ entries: [] })
    saveToStorage([])
  },
  filter: (category) => {
    const all = get().entries
    return category === 'all' ? all : all.filter((e) => e.category === category)
  },
}))
