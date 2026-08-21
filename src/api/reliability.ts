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
  created_at: number
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
}

export const getReliabilityDashboard = (days = 30) =>
  invokeWithError<ReliabilityDashboard>('get_reliability_dashboard', { days })

export const runReliabilityEvaluation = (threshold = 0.95) =>
  invokeWithError<EvalRun>('run_reliability_evaluation', { threshold })
