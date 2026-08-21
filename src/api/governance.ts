import { invokeWithError } from './invoke'

export interface ExtensionGovernanceRecord {
  extension_kind: 'skill' | 'mcp' | 'workflow'
  extension_id: string
  project_id: string | null
  source_uri: string | null
  source_revision: string | null
  content_sha256: string
  signer_key_id: string | null
  verification_state: 'unsigned' | 'verified' | 'invalid' | 'drifted'
  calls_per_minute: number
  failure_threshold: number
  cooldown_seconds: number
  consecutive_failures: number
  circuit_open_until: number | null
  last_error: string | null
  updated_at: number
}

export const listExtensionGovernance = (projectId?: string | null) =>
  invokeWithError<ExtensionGovernanceRecord[]>('list_extension_governance', { projectId: projectId ?? null })

export const configureExtensionGovernance = (
  extensionKind: ExtensionGovernanceRecord['extension_kind'], extensionId: string,
  callsPerMinute: number, failureThreshold: number, cooldownSeconds: number,
) => invokeWithError<void>('configure_extension_governance', {
  extensionKind, extensionId, callsPerMinute, failureThreshold, cooldownSeconds,
})
