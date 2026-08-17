import { useState, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { getCurrentVersion, listAvailableVersions, installVersion, type VersionInfo } from '../api/version'
import { getSystemProxy } from '../api/update'

const PROXY_KEY = 'deveco-switch-version-proxy'

export default function VersionsPage() {
  const { t } = useTranslation()
  const [current, setCurrent] = useState<string>('')
  const [versions, setVersions] = useState<VersionInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [installing, setInstalling] = useState<string | null>(null)
  // 走系统代理：默认开启，记住上次选择
  const [useProxy, setUseProxy] = useState(() => localStorage.getItem(PROXY_KEY) !== '0')
  const [systemProxy, setSystemProxy] = useState<string | null>(null)
  const [fetchError, setFetchError] = useState<string | null>(null)

  // 是否已安装过任一版本（用于区分「安装」与「切换到此版本」）
  const isInstalled =
    current !== '' && current !== t('version.notInstalled') && current !== t('version.loading')

  useEffect(() => {
    getSystemProxy()
      .then((p) => setSystemProxy(p))
      .catch(() => setSystemProxy(null))
  }, [])

  const load = async (proxy: boolean) => {
    setLoading(true)
    setFetchError(null)
    try {
      const v = await getCurrentVersion()
      setCurrent(v)
    } catch {
      setCurrent(t('version.notInstalled'))
    }
    try {
      const list = await listAvailableVersions(proxy)
      setVersions(list)
    } catch (e) {
      console.error(e)
      setFetchError(String(e))
    }
    setLoading(false)
  }

  // 挂载 + 代理开关变化时加载（函数引用每次渲染变化属预期，仅依赖开关值）
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { load(useProxy) }, [useProxy])

  const handleToggleProxy = (checked: boolean) => {
    setUseProxy(checked)
    localStorage.setItem(PROXY_KEY, checked ? '1' : '0')
  }

  const handleInstall = async (version: string) => {
    const msg = isInstalled
      ? t('version.switchConfirm', { version, current })
      : t('version.installConfirm', { version })
    if (!window.confirm(msg)) return
    setInstalling(version)
    try {
      await installVersion(version, useProxy)
      await load(useProxy)
    } catch (e) {
      alert(t('version.installFailed') + `: ${e}`)
    }
    setInstalling(null)
  }

  return (
    <div>
      <h2 className="text-xl font-semibold mb-2">{t('version.title')}</h2>
      <p className="text-sm text-[var(--text-secondary)] mb-6 leading-relaxed">{t('version.desc')}</p>

      {/* 走系统代理开关：作用于版本列表获取与安装 */}
      <div className="modern-card rounded-lg p-4 mb-6">
        <label className="flex items-center gap-2 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={useProxy}
            onChange={(e) => handleToggleProxy(e.target.checked)}
            className="accent-[var(--accent)]"
          />
          <span className="text-sm font-medium">{t('version.useProxy')}</span>
        </label>
        <p className="text-xs text-[var(--text-secondary)] mt-1 leading-relaxed">
          {t('version.useProxyHint')}
        </p>
        {useProxy && systemProxy ? (
          <p className="text-xs font-mono mt-1 text-[var(--accent)]">{t('version.proxyDetected', { proxy: systemProxy })}</p>
        ) : (
          useProxy && (
            <p className="text-xs text-[var(--text-secondary)]/80 mt-1">{t('version.proxyNotFound')}</p>
          )
        )}
      </div>

      <div className="modern-card rounded-lg p-4 mb-6">
        <p className="text-sm text-[var(--text-secondary)]">{t('version.current')}</p>
        <p className="text-lg font-mono mt-1">{current || t('version.loading')}</p>
        {!isInstalled && current !== t('version.loading') && (
          <p className="text-xs text-[var(--text-secondary)]/80 mt-2 leading-relaxed">{t('version.notInstalledHint')}</p>
        )}
      </div>

      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('version.available')}</h3>

      {fetchError && (
        <p className="text-xs text-[var(--danger)] mb-3 leading-relaxed">
          {t('version.fetchErrorHint')}
          <span className="font-mono block mt-1 break-all">{fetchError}</span>
        </p>
      )}

      {loading ? (
        <p className="text-sm text-[var(--text-secondary)]">{t('version.loading')}</p>
      ) : (
        <div className="space-y-2 max-h-96 overflow-y-auto">
          {versions.length === 0 && !fetchError && (
            <p className="text-sm text-[var(--text-secondary)]">{t('version.empty')}</p>
          )}
          {versions.map((v) => (
            <div
              key={v.version}
              className="flex items-center justify-between modern-card rounded-lg px-4 py-2"
            >
              <div className="flex items-center gap-2">
                <span className="font-mono text-sm">{v.version}</span>
                {v.is_current && (
                  <span className="text-xs px-2 py-0.5 bg-[var(--success)] text-white rounded">{t('version.currentTag')}</span>
                )}
                {v.tag && (
                  <span className="text-xs px-2 py-0.5 btn-primary rounded">{v.tag}</span>
                )}
              </div>
              {!v.is_current && (
                <button
                  onClick={() => handleInstall(v.version)}
                  disabled={installing !== null}
                  className="px-3 py-1 text-xs btn-primary rounded disabled:opacity-50 transition-colors"
                >
                  {installing === v.version
                    ? (isInstalled ? t('version.switching') : t('version.installing'))
                    : (isInstalled ? t('version.switch') : t('version.install'))}
                </button>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
