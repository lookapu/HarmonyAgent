import { invokeWithError } from './invoke'

export const readConfig = () => invokeWithError<Record<string, unknown>>('read_config')
export const writeConfig = (config: Record<string, unknown>) => invokeWithError<void>('write_config', { config })
export const getConfigPath = () => invokeWithError<string>('get_config_path')

/** 一键清空内容类数据（会话/消息/记忆/日志，保留配置与知识库）；返回 (删除会话数, 删除消息数) */
export const clearContentData = () => invokeWithError<[number, number]>('clear_content_data')
/** 立即执行滚动清理（日志/成本明细保留策略）；返回 (清理日志条数, 清理成本明细条数) */
export const runMaintenance = () => invokeWithError<[number, number]>('run_maintenance')

/** 导出数据库完整备份快照；dest 可选目标目录，返回 "路径|大小字节" */
export const exportBackup = (dest?: string) => invokeWithError<string>('export_backup', { dest })
/** 当前数据规模统计（供"数据管理"区展示） */
export interface DataScale {
  conversations: number
  messages: number
  request_logs: number
  task_runs: number
  project_memories: number
}
export const getDataScale = () => invokeWithError<DataScale>('data_scale')

// ---------------------------------------------------------------------------
// 诊断包（P2-3）
//
// 与 `exportBackup` 是**两件相反的事**，别混：
// - `exportBackup` 是**内容备份**：整库快照，含用户全部数据，留在本机
// - 诊断包是**环境证据**：版本 + 规模计数 + 近期错误，**不含用户内容**，
//   设计目标是用户敢直接发给客服
// ---------------------------------------------------------------------------

/** 诊断包预览里的单个条目 */
export interface DiagnosticEntry {
  path: string
  kind: string
  bytes: number
  sha256: string
}

/** 某个错误来源本次采集情况。`included < available` 表示被条数上限截断了。 */
export interface DiagnosticSourceCount {
  source: string
  included: number
  available: number
}

export interface DiagnosticPreview {
  schema: number
  /** 本次将要写出的确切内容的摘要。导出时必须原样带回，否则后端拒绝。 */
  preview_digest: string
  app_version: string
  entries: DiagnosticEntry[]
  total_bytes: number
  source_counts: DiagnosticSourceCount[]
  truncated: DiagnosticSourceCount[]
  generated_at: number
}

export interface DiagnosticBundleRecord {
  path: string
  bytes: number
  sha256: string
  entry_count: number
  preview_digest: string
  generated_at: number
}

/** 预览将要导出的诊断包内容。不落盘。 */
export const previewDiagnosticBundle = () =>
  invokeWithError<DiagnosticPreview>('preview_diagnostic_bundle')

/** 导出诊断包。必须先预览过，并把 `previewDigest` 原样带回。 */
export const generateDiagnosticBundle = (dest: string, previewDigest: string) =>
  invokeWithError<DiagnosticBundleRecord>('generate_diagnostic_bundle', {
    dest,
    confirmed: true,
    previewDigest,
  })
