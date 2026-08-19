import { memo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { TaskLedgerState } from '../../stores/projectStoreTypes'
import Icon from '../../icons/Icon'

/* ============ 任务账本卡（Ledger 协议）：目标/已验证/待解决/下一步，每轮实时刷新 ============
 * 后端每轮工具执行后推送 chat-ledger（finished=false），任务结束时推送最终态
 * （finished=true：完成→清空收起；中断→保留展示）。切回会话时从库恢复未完成任务账本。 */
export const LedgerCard = memo(function LedgerCard({ state }: { state: TaskLedgerState }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(true)
  const { ledger, finished } = state
  if (!ledger) return null
  const running = !finished
  return (
    <div className="overflow-hidden animate-fade-in-up">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 py-1.5 text-left hover:opacity-80 transition-opacity"
      >
        <Icon
          name="receipt"
          size={11}
          className={running ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'}
        />
        <div className="flex-1 min-w-0">
          <span className="text-[12px] text-[var(--text-secondary)]">{t('home.ledgerTitle')}</span>
          {!open && ledger.goal && (
            <span className="text-[11px] text-[var(--text-muted)] ml-2 truncate">{ledger.goal}</span>
          )}
        </div>
        <span
          className={`text-[11px] shrink-0 flex items-center gap-1 ${
            running ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'
          }`}
        >
          {running ? (
            <span className="relative flex h-1.5 w-1.5">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-[var(--accent)] opacity-75" />
              <span className="relative inline-flex rounded-full h-1.5 w-1.5 bg-[var(--accent)]" />
            </span>
          ) : null}
          {running ? t('home.ledgerRunning') : t('home.ledgerPaused')}
        </span>
        <Icon name="chevron-right" size={11} className={`text-[var(--text-muted)] transition-transform ${open ? 'rotate-90' : ''}`} />
      </button>
      {open && (
        <div className="border-t border-[var(--border)]/60 py-1.5 space-y-1">
          <div className="text-[11px] text-[var(--text-muted)] truncate mb-1" title={ledger.goal}>
            {ledger.goal}
          </div>
          {ledger.verified.length > 0 && (
            <div className="space-y-0.5">
              {ledger.verified.map((e) => (
                <div key={e.n} className="flex items-start gap-2 text-[11.5px] leading-relaxed">
                  <Icon name="check" size={11} className="text-[var(--success)] shrink-0 mt-0.5" />
                  <span className="min-w-0 flex-1">
                    <span className="text-[var(--text-muted)]">
                      #{e.n} [{e.tool}]{' '}
                    </span>
                    <span className="text-[var(--text-secondary)]">{e.text}</span>
                  </span>
                </div>
              ))}
            </div>
          )}
          {ledger.open.length > 0 && (
            <div className="space-y-0.5">
              {ledger.open.map((e) => (
                <div key={e.n} className="flex items-start gap-2 text-[11.5px] leading-relaxed">
                  <Icon name="close" size={11} className="text-[var(--danger)] shrink-0 mt-0.5" />
                  <span className="min-w-0 flex-1">
                    <span className="text-[var(--text-muted)]">
                      #{e.n} [{e.tool}]{' '}
                    </span>
                    <span className="text-[var(--text-secondary)]">{e.text}</span>
                  </span>
                </div>
              ))}
            </div>
          )}
          {ledger.next && (
            <div className="flex items-start gap-2 text-[11.5px] leading-relaxed pt-1.5 border-t border-[var(--border)]/60">
              <Icon name="bolt" size={11} className="text-[var(--accent)] shrink-0 mt-0.5" />
              <span className="min-w-0 flex-1 text-[var(--text-secondary)]">{ledger.next}</span>
            </div>
          )}
        </div>
      )}
    </div>
  )
})
