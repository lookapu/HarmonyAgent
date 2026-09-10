import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { probeSandboxBackends, type SandboxCapabilities } from '../api/sandbox'

const DISPLAY_NAMES: Record<string, string> = {
  'macos-sandbox-exec': 'macOS sandbox-exec',
  'linux-bubblewrap': 'Linux bubblewrap',
  'windows-app-container': 'Windows AppContainer',
  docker: 'Docker (OCI)',
  podman: 'Podman (OCI)',
}

export default function SandboxCapabilityPanel() {
  const { t } = useTranslation()
  const [capabilities, setCapabilities] = useState<SandboxCapabilities[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

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

  return (
    <section className="mt-8" aria-labelledby="sandbox-capability-title">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div>
          <h3 id="sandbox-capability-title" className="text-sm font-medium text-[var(--text-secondary)]">
            {t('health.sandboxTitle')}
          </h3>
          <p className="mt-1 text-[11px] text-[var(--text-muted)]">{t('health.sandboxSubtitle')}</p>
        </div>
        <button
          type="button"
          onClick={() => void load()}
          disabled={loading}
          className="rounded-lg border border-[var(--border)] px-3 py-1.5 text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-hover)] disabled:opacity-50"
        >
          {loading ? t('health.sandboxProbing') : t('health.sandboxProbe')}
        </button>
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
                  capability.resource_limits && t('health.sandboxResourceLimits'),
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
              </div>
            )
          })
        )}
      </div>
      <p className="mt-2 text-[11px] leading-5 text-[var(--warning)]">{t('health.sandboxBoundaryNote')}</p>
    </section>
  )
}
