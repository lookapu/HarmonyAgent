import { invoke } from '@tauri-apps/api/core'

export const readConfig = () => invoke<Record<string, unknown>>('read_config')
export const writeConfig = (config: Record<string, unknown>) => invoke<void>('write_config', { config })
export const getConfigPath = () => invoke<string>('get_config_path')

/** 一键清空内容类数据（会话/消息/记忆/日志，保留配置与知识库）；返回 (删除会话数, 删除消息数) */
export const clearContentData = () => invoke<[number, number]>('clear_content_data')
/** 立即执行滚动清理（日志/成本明细保留策略）；返回 (清理日志条数, 清理成本明细条数) */
export const runMaintenance = () => invoke<[number, number]>('run_maintenance')

/** 导出数据库完整备份快照；dest 可选目标目录，返回 "路径|大小字节" */
export const exportBackup = (dest?: string) => invoke<string>('export_backup', { dest })
/** 当前数据规模统计（供"数据管理"区展示） */
export interface DataScale {
  conversations: number
  messages: number
  request_logs: number
  task_runs: number
  project_memories: number
}
export const getDataScale = () => invoke<DataScale>('data_scale')
