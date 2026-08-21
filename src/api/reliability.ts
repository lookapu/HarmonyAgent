import { invokeWithError } from './invoke'

export interface NamedCount {
  name: string
  count: number
}

export interface EvalCaseResult {
  id: string
  domain: string
  expected: string
  actual: string
  passed: boolean
}

export interface EvalRun {
  eval_run_id: string
  suite: string
  platform: string
  passed: boolean
  total_cases: number
  passed_cases: number
  score: number
  threshold: number
  results: EvalCaseResult[]
  snapshot: EvalExecutionSnapshot
  created_at: number
}

export interface EvalExecutionSnapshot {
  schema_version: number
  producer_version: string
  model: { used: boolean; provider_id: string | null; model_id: string | null; protocol: string | null }
  prompt: { used: boolean; profile_version: string; digest: string; content_included: boolean }
  tools: { registry_version: string; registry_count: number; registry_digest: string; external_calls: number }
  sdk: {
    status: string
    source: string
    default_api: string | null
    variants: Array<{ variant: string; api_version: string | null; component_versions: string[]; is_default: boolean }>
    has_hdc: boolean
    has_ohpm: boolean
    has_hvigorw: boolean
  }
  device_inventory: {
    status: string
    error: string | null
    devices: Array<{
      id_digest: string
      connection: string
      authorized: boolean
      model: string
      os_version: string
      api_level: number | null
      architecture: string
      capabilities: string[]
    }>
  }
  metrics: { duration_ms: number; input_tokens: number; output_tokens: number; cost_cny: number }
  evidence: { passed_case_digests: string[]; failed_case_digests: string[]; final_digest: string }
}

export interface QualityRunRow {
  run_id: string
  conversation_id: string
  goal: string
  state: string
  score: number | null
  acceptance_passed: boolean | null
  remediation_count: number
  recovered: boolean
  updated_at: number
}

export interface ReliabilityDashboard {
  total_runs: number
  acceptance_rate: number
  average_quality_score: number
  remediation_success_rate: number
  recovery_success_rate: number
  false_completion_count: number
  structured_evidence_coverage: number
  duplicate_side_effect_count: number
  scheduler_states: NamedCount[]
  dag_total_nodes: number
  dag_completed_nodes: number
  dag_failed_nodes: number
  latest_eval: EvalRun | null
  recent_runs: QualityRunRow[]
  open_alert_count: number
  critical_alert_count: number
  quota: QuotaUsage
  worker_runtime: WorkerRuntimeStats
  tool_runtime: ToolRuntimeStats
  tool_governance: ToolGovernanceItem[]
  tool_quality: ToolQualitySummary
  tool_metric_slices: ToolMetricSlice[]
  tool_protocol_versions: ToolProtocolVersion[]
}

export interface ToolQualitySummary {
  total_calls: number
  successful_calls: number
  success_rate: number
  argument_error_rate: number
  timeout_rate: number
  retry_rate: number
  cancellation_count: number
  average_cancellation_latency_ms: number | null
  average_duration_ms: number
  contributing_success_rate: number
  side_effect_repeat_rate: number
  wrong_tool_selection_rate: number
  ineffective_call_rate: number
}

export interface ToolMetricSlice {
  dimension: 'tool' | 'capability_pack' | 'model' | 'project' | 'version' | string
  value: string
  calls: number
  successes: number
  success_rate: number
  contribution_rate: number
  average_duration_ms: number
}

export interface ToolProtocolVersion {
  schema_version: number
  status: string
  min_reader_version: number
  producer_version: string
  compatibility: string
  migration_notes: string
}

export interface ToolGovernanceItem {
  tool: string
  related_tool: string | null
  issue: 'high_failure_rate' | 'long_unused' | 'overlap_candidate' | string
  action: 'fix' | 'hide_candidate' | 'merge_review' | string
  calls: number
  failures: number
  failure_rate: number
  evidence: string
}

export interface WorkerRuntimeStats {
  active_workers: number
  lost_workers: number
  running_tasks: number
  recovered_tasks: number
}

export interface ToolRuntimeStats {
  active_workers: number
  lost_workers: number
  running_tools: number
  verification_required: number
  manual_review_required: number
  recovered_tools: number
  timed_out_tools: number
  worker_panics: number
  stuck_tools: number
}

export interface ToolExecutionWorker {
  worker_id: string
  process_worker_id: string
  pid: number
  platform: string
  state: string
  capacity: number
  active_tools: number
  started_at: number
  last_heartbeat_at: number
  stopped_at: number | null
}

export interface AgentWorker {
  worker_id: string
  worker_kind: string
  pid: number
  hostname: string
  version: string
  state: string
  capacity: number
  active_tasks: number
  started_at: number
  last_heartbeat_at: number
  draining_at: number | null
  stopped_at: number | null
}

export interface QuotaUsage {
  tenant_id: string
  period: string
  runs: number
  tool_calls: number
  failed_tools: number
  duration_ms: number
  cost_cny: number
  updated_at: number
}

export interface AgentAlert {
  alert_id: string
  run_id: string | null
  severity: string
  code: string
  message: string
  state: string
  details_json: string
  created_at: number
  resolved_at: number | null
}

export interface AuditEvent {
  audit_id: string
  run_id: string | null
  conversation_id: string | null
  actor: string
  action: string
  resource: string
  outcome: string
  details_json: string
  created_at: number
}

export interface SloPolicy {
  policy_id: string
  name: string
  enabled: boolean
  acceptance_target: number
  recovery_target: number
  evidence_target: number
  max_side_effect_repeat_rate: number
  max_wrong_tool_selection_rate: number
  max_ineffective_call_rate: number
  max_duration_ms: number
  max_cost_cny: number | null
  updated_at: number
}

export const getReliabilityDashboard = (days = 30) =>
  invokeWithError<ReliabilityDashboard>('get_reliability_dashboard', { days })

export const runReliabilityEvaluation = (threshold = 0.95) =>
  invokeWithError<EvalRun>('run_reliability_evaluation', { threshold })

export const listAgentAlerts = (limit = 100) => invokeWithError<AgentAlert[]>('list_agent_alerts', { limit })
export const listAgentAuditEvents = (runId?: string, limit = 200) =>
  invokeWithError<AuditEvent[]>('list_agent_audit_events', { runId, limit })
export const listAgentWorkers = (limit = 100) => invokeWithError<AgentWorker[]>('list_agent_workers', { limit })
export const listToolExecutionWorkers = (limit = 100) =>
  invokeWithError<ToolExecutionWorker[]>('list_tool_execution_workers', { limit })
export const getAgentSloPolicy = () => invokeWithError<SloPolicy | null>('get_agent_slo_policy')
export const updateAgentSloPolicy = (policy: SloPolicy) => invokeWithError<void>('update_agent_slo_policy', { policy })

export const resumeScheduledAgentTask = (runId: string, resumeToken: string) =>
  invokeWithError<boolean>('resume_scheduled_agent_task', { runId, resumeToken })

export const pauseScheduledAgentTask = (runId: string) =>
  invokeWithError<boolean>('pause_scheduled_agent_task', { runId })

export const cancelScheduledAgentTask = (runId: string) =>
  invokeWithError<boolean>('cancel_scheduled_agent_task', { runId })
