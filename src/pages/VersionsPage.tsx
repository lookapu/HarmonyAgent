import { useState, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { getCurrentVersion, listAvailableVersions, installVersion, type VersionInfo } from '../api/version'

export default function VersionsPage() {
  const { t } = useTranslation()
  const [current, setCurrent] = useState<string>('')
  const [versions, setVersions] = useState<VersionInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [installing, setInstalling] = useState<string | null>(null)

  const load = async () => {
    setLoading(true)
    try {
      const v = await getCurrentVersion()
      setCurrent(v)
    } catch {
      setCurrent(t('version.notInstalled'))
    }
    try {
      const list = await listAvailableVersions()
      setVersions(list)
    } catch (e) {
      console.error(e)
    }
    setLoading(false)
  }

  // 挂载时加载一次：函数引用每次渲染变化属预期，不加入依赖避免重复请求
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { load() }, [])

  const handleInstall = async (version: string) => {
    setInstalling(version)
    try {
      await installVersion(version)
      await load()
    } catch (e) {
      alert(t('version.installing') + `: ${e}`)
    }
    setInstalling(null)
  }

  return (
    <div>
      <h2 className="text-xl font-semibold mb-6">{t('version.title')}</h2>

      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4 mb-6">
        <p className="text-sm text-[var(--text-secondary)]">{t('version.current')}</p>
        <p className="text-lg font-mono mt-1">{current || t('version.loading')}</p>
      </div>

      <h3 className="text-sm font-medium text-[var(--text-secondary)] mb-3">{t('version.available')}</h3>

      {loading ? (
        <p className="text-sm text-[var(--text-secondary)]">{t('version.loading')}</p>
      ) : (
        <div className="space-y-2 max-h-96 overflow-y-auto">
          {versions.map((v) => (
            <div
              key={v.version}
              className="flex items-center justify-between bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg px-4 py-2"
            >
              <div className="flex items-center gap-2">
                <span className="font-mono text-sm">{v.version}</span>
                {v.is_current && (
                  <span className="text-xs px-2 py-0.5 bg-[var(--success)] text-white rounded">{t('version.currentTag')}</span>
                )}
                {v.tag && (
                  <span className="text-xs px-2 py-0.5 bg-[var(--accent)] text-white rounded">{v.tag}</span>
                )}
              </div>
              {!v.is_current && (
                <button
                  onClick={() => handleInstall(v.version)}
                  disabled={installing !== null}
                  className="px-3 py-1 text-xs bg-[var(--accent)] text-white rounded hover:bg-[var(--accent-hover)] disabled:opacity-50 transition-colors"
                >
                  {installing === v.version ? t('version.installing') : t('version.install')}
                </button>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
