import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  probeSandboxBackends,
  verifyNativeSandboxBoundary,
  type SandboxBoundaryReport,
  type SandboxCapabilities,
} from '../api/sandbox'

const DISPLAY_NAMES: Record<string, string> = {
  'macos-sandbox-exec': 'macOS sandbox-exec',
  'linux-bubblewrap': 'Linux bubblewrap',
  'windows-app-container': 'Windows AppContainer',
  docker: 'Docker (OCI)',
  podman: 'Podman (OCI)',
}

const CHECK_LABELS: Record<string, string> = {
  runtime_available: 'health.sandboxCheckRuntime',
  workspace_write: 'health.sandboxCheckWorkspaceWrite',
  external_read_denied: 'health.sandboxCheckExternalRead',
  read_only_write_denied: 'health.sandboxCheckReadOnly',
  symlink_escape_denied: 'health.sandboxCheckSymlink',
}

export default function SandboxCapabilityPanel() {
  const { t } = useTranslation()
  const [capabilities, setCapabilities] = useState<SandboxCapabilities[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [verifying, setVerifying] = useState(false)
  const [boundary, setBoundary] = useState<SandboxBoundaryReport | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setCapabilities(await probeSandboxBackends())
    } catch (err) {
      setError(String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const verifyBoundary = useCallback(async () => {
    setVerifying(true)
    setError(null)
    try {
      setBoundary(await verifyNativeSandboxBoundary())
    } catch (err) {
      setError(String(err))
    } finally {
      setVerifying(false)
    }
  }, [])

  return (
    <section className="mt-8" aria-labelledby="sandbox-capability-title">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div>
          <h3 id="sandbox-capability-title" className="text-sm font-medium text-[var(--text-secondary)]">
            {t('health.sandboxTitle')}
          </h3>
          <p className="mt-1 text-[11px] text-[var(--text-muted)]">{t('health.sandboxSubtitle')}</p>
        </div>
        <div className="flex shrink-0 gap-2">
          <button
            type="button"
            onClick={() => void verifyBoundary()}
            disabled={verifying}
            className="rounded-lg border border-[var(--border)] px-3 py-1.5 text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-hover)] disabled:opacity-50"
          >
            {verifying ? t('health.sandboxVerifying') : t('health.sandboxVerify')}
          </button>
          <button
            type="button"
            onClick={() => void load()}
            disabled={loading}
            className="rounded-lg border border-[var(--border)] px-3 py-1.5 text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-hover)] disabled:opacity-50"
          >
            {loading ? t('health.sandboxProbing') : t('health.sandboxProbe')}
          </button>
        </div>
      </div>

      <div className="modern-card overflow-hidden rounded-lg">
        {error && (
          <div className="border-b border-[var(--danger)]/20 bg-[var(--danger)]/10 px-4 py-3 text-xs text-[var(--danger)]">
            {t('health.sandboxProbeFailed', { err: error })}
          </div>
        )}
        {loading && capabilities.length === 0 ? (
          <div className="px-4 py-5 text-sm text-[var(--text-muted)]">{t('health.sandboxProbing')}</div>
        ) : (
          capabilities.map((capability) => {
            const enforced = capability.available
              ? [
                  capability.os_level_isolation && t('health.sandboxOsIsolation'),
                  capability.filesystem_read_only && t('health.sandboxReadOnly'),
                  capability.workspace_write && t('health.sandboxWorkspaceWrite'),
                  capability.network_none && t('health.sandboxNetworkNone'),
                  capability.wall_time_limit && t('health.sandboxWallTimeLimit'),
                  capability.output_limit && t('health.sandboxOutputLimit'),
                  capability.cpu_limit && t('health.sandboxCpuLimit'),
                  capability.memory_limit && t('health.sandboxMemoryLimit'),
                  capability.pids_limit && t('health.sandboxPidsLimit'),
                  capability.writable_tmp_limit && t('health.sandboxWritableTmpLimit'),
                ].filter(Boolean) as string[]
              : []
            const missingResourceLimits = capability.available
              ? [
                  !capability.wall_time_limit && t('health.sandboxWallTimeLimit'),
                  !capability.output_limit && t('health.sandboxOutputLimit'),
                  !capability.cpu_limit && t('health.sandboxCpuLimit'),
                  !capability.memory_limit && t('health.sandboxMemoryLimit'),
                  !capability.pids_limit && t('health.sandboxPidsLimit'),
                  !capability.writable_tmp_limit && t('health.sandboxWritableTmpLimit'),
                ].filter(Boolean) as string[]
              : []
            return (
              <div
                key={capability.backend}
                className="border-b border-[var(--border)] px-4 py-3 last:border-b-0"
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span
                        className={`h-2.5 w-2.5 shrink-0 rounded-full ${capability.available ? 'bg-[var(--success)]' : 'bg-[var(--text-muted)]'}`}
                      />
                      <span className="text-[13px] font-medium text-[var(--text-primary)]">
                        {DISPLAY_NAMES[capability.backend] ?? capability.backend}
                      </span>
                    </div>
                    <p className="mt-1 break-words pl-[18px] text-[11px] text-[var(--text-muted)]">
                      {capability.reason ?? t('health.sandboxNoDetail')}
                    </p>
                  </div>
                  <span
                    className={`shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium ${capability.available ? 'bg-[var(--success)]/10 text-[var(--success)]' : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'}`}
                  >
                    {capability.available ? t('health.sandboxAvailable') : t('health.sandboxUnavailable')}
                  </span>
                </div>
                {enforced.length > 0 && (
                  <div className="mt-2 flex flex-wrap gap-1.5 pl-[18px]">
                    {enforced.map((label) => (
                      <span key={label} className="rounded bg-[var(--bg-hover)] px-1.5 py-0.5 text-[10px] text-[var(--text-secondary)]">
                        {label}
                      </span>
                    ))}
                  </div>
                )}
                {missingResourceLimits.length > 0 && (
                  <p className="mt-2 pl-[18px] text-[10px] text-[var(--warning)]">
                    {t('health.sandboxLimitsMissing', { limits: missingResourceLimits.join(' / ') })}
                  </p>
                )}
              </div>
            )
          })
        )}
      </div>
      {boundary && (
        <div className={`mt-2 rounded-lg border px-4 py-3 ${boundary.passed ? 'border-[var(--success)]/25 bg-[var(--success)]/5' : 'border-[var(--warning)]/25 bg-[var(--warning)]/5'}`}>
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs font-medium text-[var(--text-primary)]">
              {t('health.sandboxVerifyResult')}
            </span>
            <span className={`text-[11px] font-medium ${boundary.passed ? 'text-[var(--success)]' : 'text-[var(--warning)]'}`}>
              {boundary.passed ? t('health.sandboxVerifyPassed') : t('health.sandboxVerifyFailed')}
            </span>
          </div>
          <div className="mt-2 grid gap-1.5 sm:grid-cols-2">
            {boundary.checks.map((check) => (
              <div key={check.name} className="flex items-start gap-2 text-[11px]">
                <span className={check.passed ? 'text-[var(--success)]' : 'text-[var(--danger)]'}>
                  {check.passed ? '✓' : '✕'}
                </span>
                <span className="text-[var(--text-secondary)]">
                  {t(CHECK_LABELS[check.name] ?? check.name)}
                  {!check.passed && check.detail ? ` · ${check.detail}` : ''}
                </span>
              </div>
            ))}
          </div>
          <p className="mt-2 text-[10px] text-[var(--text-muted)]">{t('health.sandboxVerifyScope')}</p>
        </div>
      )}
      <p className="mt-2 text-[11px] leading-5 text-[var(--warning)]">{t('health.sandboxBoundaryNote')}</p>
    </section>
  )
}
