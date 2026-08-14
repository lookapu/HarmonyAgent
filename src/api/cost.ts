import { invoke } from '@tauri-apps/api/core'

export interface RequestLog {
  id: string
  provider_id: string | null
  model: string | null
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_creation_tokens: number
  total_cost_cny: number
  latency_ms: number | null
  first_token_ms: number | null
  status_code: number | null
  error_message: string | null
  session_id: string | null
  is_streaming: boolean
  created_at: number
}

export interface DailyUsage {
  date: string
  provider_id: string | null
  model: string | null
  request_count: number
  input_tokens: number
  output_tokens: number
  total_cost_cny: number
}

export interface CostSummary {
  total_requests: number
  total_input_tokens: number
  total_output_tokens: number
  total_cost_cny: number
  by_provider: { provider_id: string; provider_name: string; request_count: number; total_cost_cny: number }[]
  by_model: { model: string; request_count: number; input_tokens: number; output_tokens: number; total_cost_cny: number }[]
}

export interface DateRange {
  start: string
  end: string
}

export interface LogFilter {
  limit?: number
  offset?: number
  provider_id?: string
  model?: string
}

/** 任务级指标（task_runs 聚合：成功率 / 耗时分布 / 成本 / 错误分类） */
export interface TaskStats {
  total_tasks: number
  success_count: number
  error_count: number
  cancelled_count: number
  /** 成功率 0~1（成功 / 非取消任务） */
  success_rate: number
  p50_ms: number | null
  p95_ms: number | null
  avg_duration_ms: number | null
  total_cost_cny: number
  total_input_tokens: number
  total_output_tokens: number
  /** 错误分类分布（kind -> 次数，按次数倒序） */
  by_error_kind: { kind: string; count: number }[]
}

/** 单次任务运行记录（trace 明细） */
export interface TaskRun {
  id: string
  conversation_id: string
  project_id: string
  provider_id: string | null
  model: string | null
  /** success | error | cancelled */
  status: string
  error_kind: string | null
  error_message: string | null
  tool_rounds: number
  retry_count: number
  input_tokens: number
  output_tokens: number
  cost_cny: number
  duration_ms: number
  started_at: number
  finished_at: number
}

export const getCostSummary = (range: DateRange) => invoke<CostSummary>('get_cost_summary', { range })
export const getRequestLogs = (filter: LogFilter) => invoke<RequestLog[]>('get_request_logs', { filter })
export const getDailyUsage = (range: DateRange) => invoke<DailyUsage[]>('get_daily_usage', { range })
/** 任务级指标聚合；project_id 为空 = 全局，days 缺省 30 */
export const getTaskStats = (projectId?: string, days = 30) =>
  invoke<TaskStats>('get_task_stats', { projectId: projectId ?? '', days })

/** 最近任务列表；project_id 为空 = 全局；status 可选过滤（success/error/cancelled） */
export const getTaskRuns = (projectId?: string, status?: string, limit = 20) =>
  invoke<TaskRun[]>('get_task_runs', { projectId: projectId ?? '', status: status ?? '', limit })
