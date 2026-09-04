import { useTranslation } from 'react-i18next'
import type { ProjectInspect } from '../api/project'
import Icon from '../icons/Icon'

interface Props {
  inspect: ProjectInspect
  onTrust: () => Promise<void>
  onReject: () => Promise<void>
  busy?: boolean
}

export default function TrustDialog({ inspect, onTrust, onReject, busy }: Props) {
  const { t } = useTranslation()

  return (
    <div className="modal-backdrop flex items-center justify-center">
      <div className="w-[520px] rounded-2xl glass-card animate-modal-in">
        <div className="flex items-center gap-2.5 px-5 py-4 border-b border-[var(--border)]">
          <div className="w-8 h-8 rounded-[10px] bg-[var(--warning-50)] flex items-center justify-center">
            <Icon name="info" size={16} />
          </div>
          <h2 className="text-[15px] font-semibold">{t('trust.title')}</h2>
        </div>

        <div className="p-5 space-y-4">
          {/* 项目信息卡 */}
          <div className="rounded-xl modern-card p-4 space-y-2.5">
            <div className="flex justify-between gap-4">
              <span className="text-[var(--text-secondary)] text-[12px] shrink-0 pt-px">{t('trust.path')}</span>
              <span className="font-mono text-[11px] break-all text-right leading-relaxed">{inspect.path}</span>
            </div>
            <div className="flex justify-between gap-4">
              <span className="text-[var(--text-secondary)] text-[12px] shrink-0">{t('trust.fileCount')}</span>
              <span className="text-[12px] tnum">{inspect.file_count}</span>
            </div>
            {inspect.is_harmony && (
              <div className="flex justify-between gap-4">
                <span className="text-[var(--text-secondary)] text-[12px] shrink-0">{t('trust.bundleName')}</span>
                <span className="font-mono text-[11px]">{inspect.bundle_name ?? '—'}</span>
              </div>
            )}
          </div>

          {/* 权限说明 */}
          <div className="rounded-xl modern-card p-4">
            <p className="text-[12px] text-[var(--text-secondary)] mb-3">{t('trust.whatWeCanDo')}</p>
            <ul className="space-y-2">
              {[t('trust.point1'), t('trust.point2'), t('trust.point3')].map((point) => (
                <li key={point} className="flex items-start gap-2 text-[12px] leading-relaxed">
                  <span className="mt-0.5 w-4 h-4 rounded-full bg-[var(--success-100)] flex items-center justify-center shrink-0">
                    <Icon name="check" size={10} className="text-[var(--success)]" />
                  </span>
                  <span className="text-[var(--text-secondary)]">{point}</span>
                </li>
              ))}
            </ul>
            <p className="text-[11px] text-[var(--text-muted)] mt-3 pt-3 border-t border-[var(--border)]">
              {t('trust.onceTip')}
            </p>
          </div>
        </div>

        <div className="flex justify-end gap-2 px-5 py-4 border-t border-[var(--border)]">
          <button
            onClick={onReject}
            disabled={busy}
            className="px-4 h-9 rounded-[10px] text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
          >
            {t('trust.reject')}
          </button>
          <button
            onClick={onTrust}
            disabled={busy}
            className="px-4 h-9 rounded-[10px] text-[13px] font-medium btn-primary disabled:opacity-40 transition-all"
          >
            {busy ? '…' : t('trust.confirm')}
          </button>
        </div>
      </div>
    </div>
  )
}


