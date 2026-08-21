import { invokeWithError } from './invoke'

export interface TeamSharePackage { schema: 1; package_id: string; name: string; version: string; source: { uri: string; revision: string }; memories: unknown[]; conventions: unknown[]; eval_sets: unknown[] }
export interface SharePreviewItem { kind: string; key: string; action: 'insert' | 'update' | 'conflict' | 'unchanged'; reason: string }
export interface SharePreview { package_id: string; version: string; digest: string; inserts: number; updates: number; conflicts: number; unchanged: number; items: SharePreviewItem[] }
export interface ShareImportRecord { batch_id: string; project_id: string; package_id: string; package_name: string; package_version: string; source_uri: string; source_revision: string; package_digest: string; state: 'applied' | 'reverted'; imported_at: number; reverted_at: number | null }
export interface ShareChangeRecord { change_id: string; batch_id: string; item_kind: string; stable_key: string; local_id: string | null; action: string; before_json: string | null; after_digest: string; created_at: number }
export interface TeamEvalSetRecord { id: string; stable_key: string; name: string; version: string; case_count: number; enabled: boolean; source_ref: string; updated_at: number }
export interface TeamEvalRun { set_id: string; name: string; passed: boolean; total_cases: number; passed_cases: number; results: Array<{ id: string; domain: string; expected: string; actual: string; passed: boolean }> }

export const previewTeamShare = (projectId: string, packageValue: unknown) => invokeWithError<SharePreview>('preview_team_share', { projectId, package: packageValue })
export const applyTeamShare = (projectId: string, packageValue: unknown) => invokeWithError<ShareImportRecord>('apply_team_share', { projectId, package: packageValue })
export const revertTeamShare = (projectId: string, batchId: string) => invokeWithError<number>('revert_team_share', { projectId, batchId })
export const listTeamShareImports = (projectId: string) => invokeWithError<ShareImportRecord[]>('list_team_share_imports', { projectId })
export const listTeamShareChanges = (projectId: string, batchId: string) => invokeWithError<ShareChangeRecord[]>('list_team_share_changes', { projectId, batchId })
export const exportTeamShare = (projectId: string, packageId: string, name: string, version: string, sourceUri: string, sourceRevision: string) => invokeWithError<TeamSharePackage>('export_team_share', { projectId, packageId, name, version, sourceUri, sourceRevision })
export const listTeamEvalSets = (projectId: string) => invokeWithError<TeamEvalSetRecord[]>('list_team_eval_sets', { projectId })
export const runTeamEvalSet = (projectId: string, setId: string) => invokeWithError<TeamEvalRun>('run_team_eval_set', { projectId, setId })
