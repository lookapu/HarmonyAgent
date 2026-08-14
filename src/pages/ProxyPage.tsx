import { useState, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { startProxy, stopProxy, getProxyStatus, getProxyConfig, updateProxyConfig, type ProxyStatus, type ProxyConfigInput } from '../api/proxy'

export default function ProxyPage() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<ProxyStatus | null>(null)
  const [config, setConfig] = useState<ProxyConfigInput>({
    listen_address: '127.0.0.1',
    listen_port: 15800,
    auto_failover: false,
    max_retries: 3,
    streaming_first_byte_timeout_s: 60,
    non_streaming_timeout_s: 600,
    enabled: false,
  })
  const [loading, setLoading] = useState(false)

  const loadStatus = async () => {
    try {
      const s = await getProxyStatus()
      setStatus(s)
    } catch (e) {
      console.error(e)
    }
  }

  const loadConfig = async () => {
    try {
      const c = await getProxyConfig()
      setConfig({ ...c })
    } catch (e) {
      console.error(e)
    }
  }

  useEffect(() => {
    loadStatus()
    loadConfig()
    const interval = setInterval(loadStatus, 3000)
    return () => clearInterval(interval)
  }, [])

  const handleStart = async () => {
    setLoading(true)
    try {
      await updateProxyConfig(config)
      await startProxy()
      await loadStatus()
    } catch (e) {
      alert(t('proxy.startFailed') + `: ${e}`)
    }
    setLoading(false)
  }

  const handleStop = async () => {
    setLoading(true)
    try {
      await stopProxy()
      await loadStatus()
    } catch (e) {
      alert(t('proxy.stopFailed') + `: ${e}`)
    }
    setLoading(false)
  }

  const isRunning = status?.running ?? false

  return (
    <div>
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-semibold">{t('proxy.title')}</h2>
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2">
            <div className={`w-3 h-3 rounded-full ${isRunning ? 'bg-[var(--success)]' : 'bg-[var(--text-secondary)]'}`} />
            <span className="text-sm">{isRunning ? t('proxy.running') : t('proxy.stopped')}</span>
          </div>
          {isRunning ? (
            <button
              onClick={handleStop}
              disabled={loading}
              className="px-4 py-2 bg-[var(--danger)] text-white rounded-lg text-sm hover:opacity-90 disabled:opacity-50 transition-colors"
            >
              {t('proxy.stop')}
            </button>
          ) : (
            <button
              onClick={handleStart}
              disabled={loading}
              className="px-4 py-2 bg-[var(--success)] text-white rounded-lg text-sm hover:opacity-90 disabled:opacity-50 transition-colors"
            >
              {t('proxy.start')}
            </button>
          )}
        </div>
      </div>

      {isRunning && status && (
        <div className="grid grid-cols-3 gap-4 mb-6">
          <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4">
            <p className="text-xs text-[var(--text-secondary)]">{t('proxy.listenAddr')}</p>
            <p className="text-sm font-mono mt-1">{status.listen_address}:{status.listen_port}</p>
          </div>
          <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4">
            <p className="text-xs text-[var(--text-secondary)]">{t('proxy.totalRequests')}</p>
            <p className="text-lg font-semibold mt-1">{status.total_requests}</p>
          </div>
          <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4">
            <p className="text-xs text-[var(--text-secondary)]">{t('proxy.activeProvider')}</p>
            <p className="text-sm mt-1">{status.active_provider || '-'}</p>
          </div>
        </div>
      )}

      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4 mb-6">
        <h3 className="text-sm font-medium mb-4">{t('proxy.config')}</h3>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="text-xs text-[var(--text-secondary)] block mb-1">{t('proxy.listenAddr')}</label>
            <input
              value={config.listen_address}
              onChange={(e) => setConfig({ ...config, listen_address: e.target.value })}
              disabled={isRunning}
              className="w-full px-3 py-2 bg-[var(--bg-card)] border border-[var(--border)] rounded text-sm text-[var(--text-primary)] disabled:opacity-50"
            />
          </div>
          <div>
            <label className="text-xs text-[var(--text-secondary)] block mb-1">{t('proxy.port')}</label>
            <input
              type="number"
              value={config.listen_port}
              onChange={(e) => setConfig({ ...config, listen_port: parseInt(e.target.value) || 15800 })}
              disabled={isRunning}
              className="w-full px-3 py-2 bg-[var(--bg-card)] border border-[var(--border)] rounded text-sm text-[var(--text-primary)] disabled:opacity-50"
            />
          </div>
          <div>
            <label className="text-xs text-[var(--text-secondary)] block mb-1">{t('proxy.maxRetries')}</label>
            <input
              type="number"
              value={config.max_retries}
              onChange={(e) => setConfig({ ...config, max_retries: parseInt(e.target.value) || 3 })}
              disabled={isRunning}
              className="w-full px-3 py-2 bg-[var(--bg-card)] border border-[var(--border)] rounded text-sm text-[var(--text-primary)] disabled:opacity-50"
            />
          </div>
          <div>
            <label className="text-xs text-[var(--text-secondary)] block mb-1">{t('proxy.nonStreamTimeout')}</label>
            <input
              type="number"
              value={config.non_streaming_timeout_s}
              onChange={(e) => setConfig({ ...config, non_streaming_timeout_s: parseInt(e.target.value) || 600 })}
              disabled={isRunning}
              className="w-full px-3 py-2 bg-[var(--bg-card)] border border-[var(--border)] rounded text-sm text-[var(--text-primary)] disabled:opacity-50"
            />
          </div>
          <div>
            <label className="text-xs text-[var(--text-secondary)] block mb-1">{t('proxy.streamTimeout')}</label>
            <input
              type="number"
              value={config.streaming_first_byte_timeout_s}
              onChange={(e) => setConfig({ ...config, streaming_first_byte_timeout_s: parseInt(e.target.value) || 60 })}
              disabled={isRunning}
              className="w-full px-3 py-2 bg-[var(--bg-card)] border border-[var(--border)] rounded text-sm text-[var(--text-primary)] disabled:opacity-50"
            />
          </div>
          <div className="flex items-center gap-2 pt-5">
            <input
              type="checkbox"
              checked={config.auto_failover}
              onChange={(e) => setConfig({ ...config, auto_failover: e.target.checked })}
              disabled={isRunning}
              className="w-4 h-4"
            />
            <label className="text-sm text-[var(--text-primary)]">{t('proxy.autoFailover')}</label>
          </div>
          <div className="flex items-center gap-2 pt-5">
            <input
              type="checkbox"
              checked={config.enabled}
              onChange={async (e) => {
                const enabled = e.target.checked
                setConfig({ ...config, enabled })
                try {
                  await updateProxyConfig({ enabled })
                } catch (err) {
                  alert(t('proxy.saveFailed', { err: String(err) }))
                }
              }}
              className="w-4 h-4"
            />
            <label className="text-sm text-[var(--text-primary)]">{t('proxy.autoStartLabel')}</label>
          </div>
          {config.enabled && (
            <div className="col-span-2 text-xs text-[var(--text-secondary)]">
              {t('proxy.autoStartDesc')}
            </div>
          )}
        </div>
      </div>

      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4">
        <h3 className="text-sm font-medium mb-3">{t('proxy.usage')}</h3>
        <div className="text-xs text-[var(--text-secondary)] space-y-2">
          <p>{t('proxy.usage1')}</p>
          <code className="block bg-[var(--bg-card)] px-3 py-2 rounded font-mono">
            http://{config.listen_address}:{config.listen_port}/v1
          </code>
          <p>{t('proxy.usage2')}</p>
          <p>{t('proxy.usage3')}</p>
        </div>
      </div>
    </div>
  )
}
