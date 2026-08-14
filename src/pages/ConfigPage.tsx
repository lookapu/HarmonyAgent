import { useState, useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { open } from '@tauri-apps/plugin-dialog'
import { readConfig, writeConfig, getConfigPath, clearContentData, runMaintenance, exportBackup, getDataScale, type DataScale } from '../api/config'

export default function ConfigPage() {
  const { t } = useTranslation()
  const [config, setConfig] = useState<string>('')
  const [configPath, setConfigPath] = useState<string>('')
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [scale, setScale] = useState<DataScale | null>(null)
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)

  const load = async () => {
    try {
      const path = await getConfigPath()
      setConfigPath(path)
      const data = await readConfig()
      setConfig(JSON.stringify(data, null, 2))
    } catch (e) {
      setError(t('config.loadError') + `: ${e}`)
    }
  }

  const refreshScale = async () => {
    try {
      setScale(await getDataScale())
    } catch { /* 静默失败，规模展示非关键路径 */ }
  }

  useEffect(() => {
    load()
    refreshScale()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps -- 挂载时加载一次：函数引用每次渲染变化属预期，不加入依赖避免重复请求

  const handleSave = async () => {
    setError(null)
    try {
      const parsed = JSON.parse(config)
      await writeConfig(parsed)
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch (e) {
      setError(t('config.saveError') + `: ${e}`)
    }
  }

  const handleRunMaintenance = async () => {
    setBusy(true)
    setActionError(null)
    setNote(null)
    try {
      const [logs, runs] = await runMaintenance()
      setNote(t('config.runCleanDone', { logs, runs }))
      refreshScale()
    } catch (e) {
      setActionError(t('config.clearError') + `: ${e}`)
    } finally {
      setBusy(false)
    }
  }

  const handleClearAll = async () => {
    if (!confirm(t('config.clearAllConfirm'))) return
    setBusy(true)
    setActionError(null)
    setNote(null)
    try {
      const [convs, msgs] = await clearContentData()
      setNote(t('config.clearAllDone', { convs, msgs }))
      refreshScale()
    } catch (e) {
      setActionError(t('config.clearError') + `: ${e}`)
    } finally {
      setBusy(false)
    }
  }

  const handleExport = async () => {
    try {
      const dir = await open({ directory: true, multiple: false })
      if (typeof dir !== 'string' || !dir) return
      setBusy(true)
      setActionError(null)
      setNote(null)
      try {
        const res = await exportBackup(dir)
        const [path, size] = res.split('|')
        setNote(t('config.exportDone', { path, size: (Number(size) / 1024).toFixed(0) }))
      } catch (e) {
        setActionError(t('config.clearError') + `: ${e}`)
      } finally {
        setBusy(false)
      }
    } catch {
      // 用户取消选择目录
    }
  }

  const scaleItems = scale
    ? [
        { label: t('config.scaleConv'), value: scale.conversations },
        { label: t('config.scaleMsg'), value: scale.messages },
        { label: t('config.scaleLog'), value: scale.request_logs },
        { label: t('config.scaleRun'), value: scale.task_runs },
        { label: t('config.scaleMem'), value: scale.project_memories },
      ]
    : []

  return (
    <div className="h-full flex flex-col gap-4">
      <div className="flex items-center justify-between mb-1">
        <div>
          <h2 className="text-xl font-semibold">{t('config.title')}</h2>
          <p className="text-xs text-[var(--text-secondary)] mt-1 font-mono">{configPath}</p>
        </div>
        <div className="flex items-center gap-2">
          {saved && <span className="text-xs text-[var(--success)]">{t('config.saved')}</span>}
          {error && <span className="text-xs text-[var(--danger)]">{error}</span>}
          <button
            onClick={handleSave}
            className="px-4 py-2 bg-[var(--accent)] text-white rounded-lg text-sm hover:bg-[var(--accent-hover)] transition-colors"
          >
            {t('config.save')}
          </button>
        </div>
      </div>

      <textarea
        value={config}
        onChange={(e) => setConfig(e.target.value)}
        spellCheck={false}
        className="min-h-[40vh] flex-1 w-full bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4 font-mono text-sm text-[var(--text-primary)] resize-none focus:outline-none focus:border-[var(--accent)]"
      />

      {/* 数据管理区 */}
      <div className="border border-[var(--border)] rounded-lg p-4 bg-[var(--bg-secondary)]">
        <h3 className="text-sm font-semibold mb-1">{t('config.dataSection')}</h3>
        <p className="text-xs text-[var(--text-secondary)] mb-3">{t('config.dataDesc')}</p>

        {scaleItems.length > 0 && (
          <div className="flex flex-wrap gap-2 mb-3">
            {scaleItems.map((item) => (
              <div
                key={item.label}
                className="px-3 py-1.5 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] flex items-center gap-2"
              >
                <span className="text-xs text-[var(--text-secondary)]">{item.label}</span>
                <span className="text-sm font-semibold text-[var(--text-primary)]">{item.value}</span>
              </div>
            ))}
          </div>
        )}

        {note && <p className="text-xs text-[var(--success)] mb-2">{note}</p>}
        {actionError && <p className="text-xs text-[var(--danger)] mb-2">{actionError}</p>}

        <div className="flex flex-wrap gap-2">
          <button
            onClick={handleRunMaintenance}
            disabled={busy}
            className="px-4 py-2 border border-[var(--border)] text-[var(--text-primary)] rounded-lg text-sm hover:bg-[var(--bg-muted)] transition-colors disabled:opacity-50"
          >
            {t('config.runClean')}
          </button>
          <button
            onClick={handleExport}
            disabled={busy}
            className="px-4 py-2 border border-[var(--border)] text-[var(--text-primary)] rounded-lg text-sm hover:bg-[var(--bg-muted)] transition-colors disabled:opacity-50"
          >
            {t('config.exportBackup')}
          </button>
          <button
            onClick={handleClearAll}
            disabled={busy}
            className="px-4 py-2 border border-[var(--danger)] text-[var(--danger)] rounded-lg text-sm hover:bg-[var(--danger)] hover:text-white transition-colors disabled:opacity-50"
            title={t('config.clearAllDesc')}
          >
            {t('config.clearAll')}
          </button>
        </div>
      </div>
    </div>
  )
}
