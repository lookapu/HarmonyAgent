import { useState, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { checkWithProxy, withProxy } from '../api/updateProxy'
import { checkBaseUpdate } from '../api/version'

export default function UpdateChecker() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [updateAvailable, setUpdateAvailable] = useState(false)
  const [version, setVersion] = useState('')
  const [downloading, setDownloading] = useState(false)
  const [dismissed, setDismissed] = useState(false)
  const [baseUpdate, setBaseUpdate] = useState<{ current: string; latest: string } | null>(null)
  const [baseDismissed, setBaseDismissed] = useState(false)

  const checkForUpdate = async () => {
    try {
      const update = await withProxy(checkWithProxy)
      if (update) {
        setUpdateAvailable(true)
        setVersion(update.version)
      }
    } catch (e) {
      console.error('Update check failed:', e)
    }
  }

  const checkBase = async () => {
    try {
      const info = await checkBaseUpdate()
      if (info.can_update) {
        setBaseUpdate({ current: info.current, latest: info.latest })
      }
    } catch (e) {
      // 基座未安装或 npm 不可达时静默，不打扰用户
      console.debug('Base update check failed:', e)
    }
  }

  useEffect(() => {
    checkForUpdate()
    checkBase()
  }, [])

  const handleUpdate = async () => {
    setDownloading(true)
    try {
      const update = await withProxy(checkWithProxy)
      if (update) {
        // 下载+安装同样置于代理窗口内（显式 proxy 已随 check 传入，环境变量注入为双保险）
        await withProxy(async () => {
          await update.downloadAndInstall()
        })
      }
    } catch (e) {
      alert(t('update.failed') + `: ${e}`)
      setDownloading(false)
    }
  }

  if (updateAvailable && !dismissed) {
    return (
      <div className="fixed bottom-4 right-4 glass-card border-[var(--accent)] rounded-lg p-4 z-50 max-w-sm animate-modal-in">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-sm font-medium tnum">{t('update.newVersion')} v{version}</p>
            <p className="text-xs text-[var(--text-secondary)] mt-1">{t('update.recommend')}</p>
          </div>
          <button
            onClick={() => setDismissed(true)}
            className="text-[var(--text-secondary)] hover:text-[var(--text-primary)] text-lg leading-none"
          >
            ×
          </button>
        </div>
        <div className="flex gap-2 mt-3">
          <button
            onClick={handleUpdate}
            disabled={downloading}
            className="px-3 py-1.5 btn-primary rounded text-xs disabled:opacity-50 transition-colors"
          >
            {downloading ? t('update.downloading') : t('update.now')}
          </button>
          <button
            onClick={() => setDismissed(true)}
            className="px-3 py-1.5 btn-ghost rounded text-xs transition-colors"
          >
            {t('update.later')}
          </button>
        </div>
      </div>
    )
  }

  if (baseUpdate && !baseDismissed) {
    return (
      <div className="fixed bottom-4 right-4 glass-card border-[var(--accent)] rounded-lg p-4 z-50 max-w-sm animate-modal-in">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-sm font-medium tnum">{t('update.baseNewVersion', { version: baseUpdate.latest })}</p>
            <p className="text-xs text-[var(--text-secondary)] mt-1 tnum">
              {t('update.baseCurrent', { version: baseUpdate.current || '-' })}
            </p>
          </div>
          <button
            onClick={() => setBaseDismissed(true)}
            className="text-[var(--text-secondary)] hover:text-[var(--text-primary)] text-lg leading-none"
          >
            ×
          </button>
        </div>
        <div className="flex gap-2 mt-3">
          <button
            onClick={() => {
              setBaseDismissed(true)
              navigate('/versions')
            }}
            className="px-3 py-1.5 btn-primary rounded text-xs transition-colors"
          >
            {t('update.baseGoInstall')}
          </button>
          <button
            onClick={() => setBaseDismissed(true)}
            className="px-3 py-1.5 btn-ghost rounded text-xs transition-colors"
          >
            {t('update.later')}
          </button>
        </div>
      </div>
    )
  }

  return null
}

