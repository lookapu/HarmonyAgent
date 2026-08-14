import { useMemo, useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { useTranslation } from 'react-i18next'
import {
  inspectProject,
  scanWorkspaceModules,
  MODULE_KIND_LABELS,
  type ProjectInspect,
  type ScannedModuleEntry,
  type ModuleKind,
} from '../api/project'
import Icon from '../icons/Icon'

interface Props {
  onConfirm: (path: string) => Promise<void>
  onClose: () => void
}

const KIND_COLOR: Record<ModuleKind, string> = {
  harmony: 'bg-[#e6f7ef] text-[#1a9b5c] dark:bg-[#1a9b5c]/15 dark:text-[#4ade80]',
  vue: 'bg-[#e6fbf3] text-[#42b883] dark:bg-[#42b883]/15 dark:text-[#4ade80]',
  react: 'bg-[#e6f4fe] text-[#149eca] dark:bg-[#149eca]/15 dark:text-[#61dafb]',
  angular: 'bg-[#ffe9e9] text-[#dd0031] dark:bg-[#dd0031]/15 dark:text-[#ff6b6b]',
  node: 'bg-[#e9f9e3] text-[#5fa04e] dark:bg-[#5fa04e]/15 dark:text-[#86efac]',
  java: 'bg-[#fff3e0] text-[#e76f00] dark:bg-[#e76f00]/15 dark:text-[#fbbf24]',
  kotlin: 'bg-[#fff3e0] text-[#e76f00] dark:bg-[#e76f00]/15 dark:text-[#fbbf24]',
  go: 'bg-[#e3f2fd] text-[#00add8] dark:bg-[#00add8]/15 dark:text-[#38bdf8]',
  python: 'bg-[#fef3c7] text-[#d97706] dark:bg-[#d97706]/15 dark:text-[#fbbf24]',
  rust: 'bg-[#fce7e7] text-[#ce422b] dark:bg-[#ce422b]/15 dark:text-[#f87171]',
  dotnet: 'bg-[#f3e8ff] text-[#7c3aed] dark:bg-[#7c3aed]/15 dark:text-[#c4b5fd]',
  flutter: 'bg-[#e7f0ff] text-[#02569b] dark:bg-[#02569b]/15 dark:text-[#60a5fa]',
  android: 'bg-[#e3f9e5] text-[#3ddc84] dark:bg-[#3ddc84]/15 dark:text-[#4ade80]',
  ios: 'bg-[var(--bg-hover)] text-[var(--text-secondary)]',
  html: 'bg-[#fff0e6] text-[#e34c26] dark:bg-[#e34c26]/15 dark:text-[#fb923c]',
  php: 'bg-[#f0e9ff] text-[#777bb4] dark:bg-[#777bb4]/15 dark:text-[#c4b5fd]',
  ruby: 'bg-[#ffe9ec] text-[#cc342d] dark:bg-[#cc342d]/15 dark:text-[#f87171]',
  cpp: 'bg-[#e6f0ff] text-[#00599c] dark:bg-[#00599c]/15 dark:text-[#60a5fa]',
  unknown: 'bg-[var(--bg-hover)] text-[var(--text-muted)]',
}

export default function AddProjectDialog({ onConfirm, onClose }: Props) {
  const { t } = useTranslation()
  const [inspect, setInspect] = useState<ProjectInspect | null>(null)
  const [modules, setModules] = useState<ScannedModuleEntry[]>([])
  const [scanning, setScanning] = useState(false)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  const pickFolder = async () => {
    setError('')
    const selected = await open({ directory: true, multiple: false, title: t('dialog.addProject') })
    if (!selected || Array.isArray(selected)) return
    setInspect(null)
    setModules([])
    setScanning(true)
    try {
      // 探测所选目录自身 + 扫描其下各类型子工程（根目录始终作为一个项目添加）
      const [info, hits] = await Promise.all([
        inspectProject(selected),
        scanWorkspaceModules(selected),
      ])
      setInspect(info)
      setModules(hits)
    } catch (e) {
      setError(String(e))
    } finally {
      setScanning(false)
    }
  }

  const confirm = async () => {
    if (!inspect) return
    setBusy(true)
    try {
      await onConfirm(inspect.path)
      onClose()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  // 顶部类型标签（优先级：模块数量 > 根是否鸿蒙）：
  // - 识别到多个子模块 → 多模块工作区
  // - 仅 1 个子模块 → 显示该子模块类型
  // - 0 个子模块：根是鸿蒙 → HarmonyOS；否则 → 普通目录
  const typeBadge = useMemo(() => {
    if (!inspect) return null
    if (modules.length > 1) {
      return { text: t('dialog.multiModule', { count: modules.length }), cls: 'bg-[var(--accent)]/15 text-[var(--accent)]' }
    }
    if (modules.length === 1) {
      const k = modules[0].kind
      return { text: MODULE_KIND_LABELS[k], cls: KIND_COLOR[k] }
    }
    if (inspect.is_harmony) {
      return { text: `HarmonyOS${inspect.app_name ? ` · ${inspect.app_name}` : ''}`, cls: KIND_COLOR.harmony }
    }
    return { text: t('dialog.genericProject'), cls: 'bg-[var(--warning)]/15 text-[var(--warning)]' }
  }, [inspect, modules, t])

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm" onClick={onClose}>
      <div
        className="w-[560px] max-h-[85vh] flex flex-col rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl shadow-black/40 animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2.5 px-5 py-4 border-b border-[var(--border)]">
          <div className="w-8 h-8 rounded-[10px] bg-[var(--accent-soft)] flex items-center justify-center">
            <Icon name="folder" size={16} />
          </div>
          <h2 className="text-[15px] font-semibold flex-1">{t('dialog.addProject')}</h2>
          <button
            onClick={onClose}
            className="p-1.5 rounded-lg text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            aria-label="close"
          >
            <Icon name="close" size={15} />
          </button>
        </div>

        <div className="p-5 space-y-4 overflow-y-auto">
          <button
            onClick={pickFolder}
            className={`w-full flex items-center gap-2.5 px-4 py-3.5 rounded-xl border text-[13px] transition-colors ${
              inspect
                ? 'border-[var(--accent)]/40 bg-[var(--accent-soft)] text-[var(--text-primary)]'
                : 'border-dashed border-[var(--border-strong)] text-[var(--text-secondary)] hover:border-[var(--accent)] hover:text-[var(--text-primary)] hover:bg-[var(--accent-soft)]'
            }`}
          >
            <Icon name="folder" size={17} className={inspect ? '' : 'opacity-60'} />
            <span className="flex-1 text-left truncate font-mono text-xs">
              {inspect ? inspect.path : t('dialog.pickFolder')}
            </span>
            <Icon name="search" size={15} className="opacity-40" />
          </button>

          {scanning && (
            <div className="text-[12px] text-[var(--text-secondary)] flex items-center gap-2">
              <span className="inline-block w-3 h-3 border-2 border-[var(--accent)] border-t-transparent rounded-full animate-spin" />
              {t('dialog.scanning')}
            </div>
          )}

          {error && (
            <p className="text-[12px] text-[var(--danger)] bg-[var(--danger)]/10 border border-[var(--danger)]/20 rounded-lg px-3 py-2 break-all">
              {error}
            </p>
          )}

          {inspect && typeBadge && (
            <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-4 space-y-2.5 text-[13px] animate-fade-in-up">
              <div className="flex items-center gap-2">
                <span className="text-[var(--text-secondary)] w-16 shrink-0">{t('dialog.projectName')}</span>
                <span className="font-medium truncate">{inspect.name}</span>
              </div>
              <div className="flex items-center gap-2">
                <span className="text-[var(--text-secondary)] w-16 shrink-0">{t('dialog.projectType')}</span>
                <span className={`px-2 py-0.5 rounded-full text-[11px] font-medium ${typeBadge.cls}`}>
                  {typeBadge.text}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <span className="text-[var(--text-secondary)] w-16 shrink-0">{t('dialog.fileCount')}</span>
                <span>{inspect.file_count}</span>
                {inspect.has_git && (
                  <span className="px-2 py-0.5 rounded-full text-[11px] bg-[var(--accent)]/15 text-[var(--accent)] font-medium">
                    git
                  </span>
                )}
              </div>
              {inspect.bundle_name && (
                <div className="flex items-center gap-2">
                  <span className="text-[var(--text-secondary)] w-16 shrink-0">bundleName</span>
                  <span className="font-mono text-xs break-all">{inspect.bundle_name}</span>
                </div>
              )}
              {inspect.already_added && (
                <p className="text-[12px] text-[var(--warning)] bg-[var(--warning)]/10 rounded-lg px-3 py-1.5">
                  {t('dialog.alreadyAdded')}
                </p>
              )}
            </div>
          )}

          {/* 识别到的各类型子工程：根目录作为一个项目添加，同时记录这些模块供联动 */}
          {!scanning && inspect && modules.length > 0 && (
            <div className="rounded-xl border border-[var(--accent)]/25 bg-[var(--accent)]/5 overflow-hidden animate-fade-in-up">
              <div className="flex items-center gap-2 px-4 py-2.5 border-b border-[var(--border)]">
                <Icon name="check" size={14} className="text-[var(--success)]" />
                <span className="text-[12px] font-medium">
                  {t('dialog.modulesFound', { count: modules.length })}
                </span>
              </div>
              <div className="max-h-[220px] overflow-y-auto">
                {modules.map((s) => (
                  <div
                    key={s.rel_path}
                    className="flex items-center gap-2 px-4 py-2 border-b border-[var(--border)] last:border-b-0"
                  >
                    <Icon name="folder" size={13} className="text-[var(--accent)] shrink-0" />
                    <div className="min-w-0 flex-1">
                      <div className="font-mono text-[11px] text-[var(--text-primary)] truncate" title={s.rel_path}>
                        {s.rel_path}
                      </div>
                    </div>
                    <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium shrink-0 ${KIND_COLOR[s.kind]}`}>
                      {MODULE_KIND_LABELS[s.kind]}
                    </span>
                  </div>
                ))}
              </div>
              <div className="px-4 py-2 text-[11px] text-[var(--text-muted)] bg-[var(--bg-secondary)] border-t border-[var(--border)]">
                {t('dialog.modulesHint')}
              </div>
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2 px-5 py-4 border-t border-[var(--border)]">
          <button
            onClick={onClose}
            className="px-4 h-9 rounded-[10px] text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {t('dialog.cancel')}
          </button>
          <button
            onClick={confirm}
            disabled={!inspect || inspect.already_added || busy || scanning}
            className="px-4 h-9 rounded-[10px] text-[13px] font-medium bg-[var(--accent)] text-white hover:bg-[var(--accent-hover)] disabled:opacity-35 disabled:cursor-not-allowed transition-all active:scale-[0.98]"
          >
            {busy ? '…' : t('dialog.confirmAdd')}
          </button>
        </div>
      </div>
    </div>
  )
}
