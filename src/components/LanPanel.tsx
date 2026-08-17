import { useState, useEffect, useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import { getLanServerStatus, startLanServer, stopLanServer, updateLanServerConfig, type LanStatusInfo } from '../api/lan'

/**
 * 局域网访问服务面板（挂在设置页顶部 / 独立 LAN 页）。
 * - 开关 → start/stop（enabled 持久化，随应用启动自动开启）
 * - 端口 / 只读模式 / 访问地址
 * - 令牌管理已拆分为独立卡片 LanTokenPanel
 */
export default function LanPanel() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<LanStatusInfo | null>(null)
  const [portInput, setPortInput] = useState('12345')
  const [readOnly, setReadOnly] = useState(false)
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const s = await getLanServerStatus()
      setStatus(s)
      setPortInput(String(s.listen_port || 12345))
      setReadOnly(s.read_only)
    } catch {
      /* 状态加载失败静默，面板保持初始态 */
    }
  }, [])

  useEffect(() => {
    load()
  }, [load])

  const run = async (fn: () => Promise<unknown>, failKey: string): Promise<boolean> => {
    setBusy(true)
    setErr(null)
    setNote(null)
    try {
      await fn()
      await load()
      return true
    } catch (e) {
      setErr(`${t(failKey)}: ${e}`)
      return false
    } finally {
      setBusy(false)
    }
  }

  const handleToggle = async () => {
    if (!status) return
    if (status.running) {
      await run(() => stopLanServer(), 'config.lanStopFailed')
    } else {
      await run(() => startLanServer(), 'config.lanStartFailed')
    }
  }

  const handleSavePort = async () => {
    const port = Number(portInput)
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setErr(t('config.lanError') + ': ' + t('config.lanPort'))
      return
    }
    const ok = await run(() => updateLanServerConfig({ port }), 'config.lanSaveFailed')
    if (ok) setNote(t('config.lanSaveDone'))
  }

  const handleToggleReadOnly = async () => {
    await run(() => updateLanServerConfig({ read_only: !readOnly }), 'config.lanSaveFailed')
  }

  const port = Number(portInput) || 0

  return (
    <div className="border border-[var(--border)] rounded-lg p-4 bg-[var(--bg-secondary)]">
      <h3 className="text-sm font-semibold mb-1">{t('config.lanSection')}</h3>
      <p className="text-xs text-[var(--text-secondary)] mb-3">{t('config.lanDesc')}</p>

      {/* 状态 + 开关 */}
      <div className="flex items-center gap-3 mb-3">
        <span
          className={`text-xs px-2 py-0.5 rounded-full ${
            status?.running
              ? 'bg-[var(--success)]/15 text-[var(--success)]'
              : 'bg-[var(--bg-muted)] text-[var(--text-secondary)]'
          }`}
        >
          {status?.running ? t('config.lanRunning') : t('config.lanStopped')}
        </span>
        <button
          onClick={handleToggle}
          disabled={busy || !status}
          className="px-4 py-1.5 btn-primary rounded-lg text-sm disabled:opacity-50"
        >
          {status?.running ? t('config.lanStop') : t('config.lanStart')}
        </button>
        {!status?.token_set && (
          <span className="text-xs text-[var(--warning)]">{t('config.lanNoToken')}</span>
        )}
      </div>

      {/* 端口 / 只读 */}
      <div className="flex flex-wrap items-center gap-4 mb-2">
        <label className="flex items-center gap-2 text-sm text-[var(--text-secondary)]">
          {t('config.lanPort')}
          <input
            type="number"
            min={1}
            max={65535}
            value={portInput}
            disabled={status?.running || busy}
            onChange={(e) => setPortInput(e.target.value)}
            className="w-24 px-2 py-1 rounded-md border border-[var(--border)] bg-[var(--bg-primary)] text-sm text-[var(--text-primary)] disabled:opacity-50"
          />
        </label>
        <button
          onClick={handleSavePort}
          disabled={status?.running || busy}
          className="px-3 py-1.5 border border-[var(--border)] rounded-lg text-xs hover:bg-[var(--bg-muted)] disabled:opacity-50"
        >
          {t('config.save')}
        </button>
        <label
          className="flex items-center gap-2 text-sm text-[var(--text-secondary)] cursor-pointer"
          title={t('config.lanReadOnlyDesc')}
        >
          <input
            type="checkbox"
            checked={readOnly}
            onChange={handleToggleReadOnly}
            disabled={busy}
            className="accent-[var(--accent)]"
          />
          {t('config.lanReadOnly')}
        </label>
      </div>
      {status?.running && (
        <p className="text-xs text-[var(--text-secondary)] mb-2">
          {t('config.lanPort')} {t('config.lanSaveDone')}后需重启服务生效
        </p>
      )}

      {/* 访问地址 */}
      {status && status.ips.length > 0 && (
        <div className="mb-3">
          <div className="text-sm text-[var(--text-secondary)] mb-1">{t('config.lanAccessUrl')}</div>
          <div className="flex flex-wrap gap-2">
            {status.ips.map((ip) => (
              <code
                key={ip}
                className="px-2 py-1 rounded-md bg-[var(--bg-primary)] border border-[var(--border)] font-mono text-xs"
              >
                http://{ip}:{port}/
              </code>
            ))}
          </div>
        </div>
      )}
      {status && status.ips.length === 0 && (
        <p className="text-xs text-[var(--warning)] mb-2">{t('config.lanNoIps')}</p>
      )}

      <p className="text-xs text-[var(--text-secondary)]">{t('config.lanFirewallHint')}</p>
      {note && <p className="text-xs text-[var(--success)] mt-1">{note}</p>}
      {err && <p className="text-xs text-[var(--danger)] mt-1">{err}</p>}
    </div>
  )
}
