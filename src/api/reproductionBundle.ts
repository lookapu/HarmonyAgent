import { invokeWithError } from './invoke'

export interface ReproductionRequest {
  title: string
  description: string
  steps: string[]
  expected: string
  actual: string
  conversation_id?: string | null
  run_id?: string | null
  include_messages: boolean
  include_tool_runs: boolean
  include_run_events: boolean
  attachments: string[]
}

export interface ReproductionEntryPreview { path: string; kind: string; bytes: number; sha256: string; redacted: boolean }
export interface ReproductionPreview { schema: number; title: string; preview_digest: string; conversation_id: string | null; run_id: string | null; entries: ReproductionEntryPreview[]; total_bytes: number; redacted_entry_count: number; omitted_attachments: string[]; warnings: string[] }
export interface ReproductionBundleRecord { bundle_id: string; project_id: string; conversation_id: string | null; run_id: string | null; title: string; preview_digest: string; archive_rel_path: string; archive_sha256: string; archive_bytes: number; entry_count: number; redacted_entry_count: number; generated_at: number }
export interface ArchiveValidation { valid: boolean; bundle_id: string; entry_count: number; preview_digest: string }

export const previewReproductionBundle = (projectId: string, request: ReproductionRequest) =>
  invokeWithError<ReproductionPreview>('preview_reproduction_bundle', { projectId, request })
export const generateReproductionBundle = (projectId: string, request: ReproductionRequest, previewDigest: string) =>
  invokeWithError<ReproductionBundleRecord>('generate_reproduction_bundle', { projectId, request, confirmed: true, previewDigest })
export const listReproductionBundles = (projectId: string) =>
  invokeWithError<ReproductionBundleRecord[]>('list_reproduction_bundles', { projectId })
export const validateReproductionBundle = (projectId: string, bundleId: string) =>
  invokeWithError<ArchiveValidation>('validate_reproduction_bundle', { projectId, bundleId })
