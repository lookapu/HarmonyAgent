import { create } from 'zustand'
import type { ProjectState } from './projectStoreTypes'
import { createProjectSlice } from './slices/projectSlice'
import { createChatSlice } from './slices/chatSlice'
import { createMemorySlice } from './slices/memorySlice'

export * from './projectStoreTypes'

/**
 * 组合状态（zustand slices）：
 * - ProjectSlice：项目/文件树/分支（slices/projectSlice.ts）
 * - ChatSlice：会话/消息/流式/审批/计划（slices/chatSlice.ts）
 * - MemorySlice：记忆/统计/反馈/版本（slices/memorySlice.ts）
 */
export const useProjectStore = create<ProjectState>()((...a) => ({
  ...createProjectSlice(...a),
  ...createChatSlice(...a),
  ...createMemorySlice(...a),
}))
