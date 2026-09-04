import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useProjectStore } from '../../stores/projectStore'
import type { MessageVersion, ChatMessage, MemoryDraft } from '../../api/project'
import Icon from '../../icons/Icon'
import { diffWords, type Change } from 'diff'
import { ThumbDownIcon } from './messageBlocks'

/* ============ 点踩反馈弹窗（可选原因） ============ */
export function FeedbackDialog({ onSubmit, onCancel }: { onSubmit: (reason?: string) => void; onCancel: () => void }) {
  const { t } = useTranslation()
  const [reason, setReason] = useState('')
  const reasons = ['内容不准确', '代码有错误', '没有帮助', '语气不合适', '其他']
  return (
    <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
      <div className="w-[420px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
        <div className="flex items-center gap-2">
          <ThumbDownIcon filled />
          <span className="text-[13px] font-semibold">{t('home.feedbackTitle')}</span>
        </div>
        <div className="mt-3 flex flex-wrap gap-1.5">
          {reasons.map((r) => (
            <button
              key={r}
              onClick={() => setReason(r)}
              className={`px-2.5 py-1 rounded-lg text-[11px] border transition-colors ${reason === r ? 'border-[var(--danger)] text-[var(--danger)] bg-[var(--danger)]/10' : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--text-muted)]'}`}
            >
              {r}
            </button>
          ))}
        </div>
        <textarea
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          placeholder={t('home.feedbackPlaceholder')}
          rows={3}
          className="mt-3 w-full resize-none rounded-lg modern-card border-[var(--border)] px-3 py-2 text-[12px] outline-none focus:border-[var(--accent)] placeholder:text-[var(--text-muted)]"
        />
        <div className="flex items-center justify-end gap-2 mt-4">
          <button
            onClick={onCancel}
            className="h-8 px-4 rounded-lg border border-[var(--border)] text-[12px] font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {t('home.cancel')}
          </button>
          <button
            onClick={() => onSubmit(reason || undefined)}
            className="h-8 px-4 rounded-lg bg-[var(--danger)] text-white text-[12px] font-medium hover:opacity-90 transition-all"
          >
            {t('home.feedbackSubmit')}
          </button>
        </div>
      </div>
    </div>
  )
}

/* ============ 回复版本 diff 对比弹窗（重新生成保留的旧回复 vs 当前） ============ */
export function VersionDiffDialog({
  userMessageId,
  current,
  onClose,
}: {
  userMessageId: string
  current: string
  onClose: () => void
}) {
  const { t } = useTranslation()
  // ⚠️ ?? [] 必须在 selector 外（否则无限重渲染，见 Home.tsx PinnedBar 注释）
  const versions = useProjectStore((s) => s.versionMap[userMessageId]) ?? []
  const [selected, setSelected] = useState<MessageVersion | null>(versions[0] ?? null)
  if (!selected) {
    return (
      <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
        <div className="w-[560px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
          <p className="text-[12px] text-[var(--text-muted)]">{t('home.noVersions')}</p>
          <button onClick={onClose} className="mt-3 h-8 px-4 rounded-lg btn-primary text-[12px]">
            {t('home.close')}
          </button>
        </div>
      </div>
    )
  }
  // diff：当前回复 vs 旧版本（旧 → 新 方向，添加为绿、删除为红）
  const changes: Change[] = diffWords(selected.content, current)
  return (
    <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]" onClick={onClose}>
      <div
        className="w-[720px] max-w-[94vw] max-h-[80vh] flex flex-col rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between shrink-0">
          <div className="flex items-center gap-2">
            <Icon name="git-branch" size={14} />
            <span className="text-[13px] font-semibold">{t('home.versionCompare')}</span>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-lg text-[var(--text-muted)] hover:bg-[var(--bg-hover)] transition-colors">
            <Icon name="close" size={15} />
          </button>
        </div>
        {/* 版本选择器（按时间倒序：最新旧版在前） */}
        <div className="flex items-center gap-1.5 mt-3 shrink-0 flex-wrap">
          {[...versions].reverse().map((v) => (
            <button
              key={v.id}
              onClick={() => setSelected(v)}
              className={`px-2.5 py-1 rounded-lg text-[11px] border transition-colors ${selected.id === v.id ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]' : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--text-muted)]'}`}
            >
              {t('home.versionLabel', { n: new Date(v.created_at * 1000).toLocaleString() })}
            </button>
          ))}
          <span className="px-2.5 py-1 rounded-lg text-[11px] border border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]">
            {t('home.versionCurrent')}
          </span>
        </div>
        <div className="mt-3 flex-1 min-h-0 overflow-y-auto rounded-xl modern-card border-[var(--border)] p-3 text-[12px] leading-relaxed whitespace-pre-wrap break-words">
          {changes.map((ch, i) => {
            const cls = ch.added
              ? 'bg-[var(--success)]/15 text-[var(--success)]'
              : ch.removed
                ? 'bg-[var(--danger)]/15 text-[var(--danger)] line-through decoration-[var(--danger)]/60'
                : 'text-[var(--text-primary)]'
            return (
              <span key={i} className={cls}>
                {ch.value}
              </span>
            )
          })}
        </div>
        <div className="mt-3 flex items-center gap-3 text-[10px] text-[var(--text-muted)] shrink-0">
          <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-sm bg-[var(--success)]/50" />{t('home.diffAdded')}</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-sm bg-[var(--danger)]/50" />{t('home.diffRemoved')}</span>
          <span className="ml-auto">{t('home.diffHint')}</span>
        </div>
      </div>
    </div>
  )
}

/* ============ 记忆总结确认弹窗（LLM 草稿 → 人工确认后落库） ============ */
export function MemoryDraftDialog({
  draft,
  onCancel,
  onConfirm,
}: {
  draft: MemoryDraft
  onCancel: () => void
  onConfirm: () => void
}) {
  const { t } = useTranslation()
  const categories = [
    ['general', t('home.memoryCat.general')],
    ['code', t('home.memoryCat.code')],
    ['build', t('home.memoryCat.build')],
    ['deploy', t('home.memoryCat.deploy')],
    ['decision', t('home.memoryCat.decision')],
    ['pitfall', t('home.memoryCat.pitfall')],
    ['path', t('home.memoryCat.path')],
  ] as const
  return (
    <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
      <div className="w-[520px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
        <div className="flex items-center gap-2">
          <Icon name="lightbulb" size={15} />
          <span className="text-[13px] font-semibold">{t('home.memoryDraftTitle')}</span>
        </div>
        <div className="mt-3 space-y-2.5">
          <div className="flex items-center gap-2">
            <span className="text-[11px] text-[var(--text-muted)] shrink-0 w-14">{t('home.memoryTitle')}</span>
            <span className="flex-1 text-[13px] font-medium">{draft.title}</span>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-[11px] text-[var(--text-muted)] shrink-0 w-14">{t('home.memoryCategory')}</span>
            <span className="px-2 py-0.5 rounded-md bg-[var(--accent-soft)] text-[var(--accent)] text-[11px]">
              {categories.find(([v]) => v === draft.category)?.[1] ?? draft.category}
            </span>
          </div>
          <div className="rounded-xl modern-card border-[var(--border)] p-3 max-h-52 overflow-y-auto">
            <p className="text-[12px] leading-relaxed text-[var(--text-primary)] whitespace-pre-wrap">{draft.content}</p>
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 mt-4">
          <button
            onClick={onCancel}
            className="h-8 px-4 rounded-lg border border-[var(--border)] text-[12px] font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {t('home.cancel')}
          </button>
          <button
            onClick={onConfirm}
            className="h-8 px-4 rounded-lg btn-primary text-[12px] font-medium transition-all"
          >
            {t('home.memoryDraftSave')}
          </button>
        </div>
      </div>
    </div>
  )
}

/* ============ 编辑消息弹窗（user 消息可编辑，保存后同步刷新会话） ============ */
export function EditMessageDialog({
  message,
  onCancel,
  onSubmit,
}: {
  message: ChatMessage
  onCancel: () => void
  onSubmit: (content: string) => void
}) {
  const { t } = useTranslation()
  const [value, setValue] = useState(message.content)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  useEffect(() => {
    // value 初始值即 message.content，挂载时用初始长度定位光标即可（message 在本弹窗生命周期内不变）
    inputRef.current?.focus()
    inputRef.current?.setSelectionRange(message.content.length, message.content.length)
  }, [message.content.length])
  return (
    <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
      <div className="w-[560px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
        <div className="flex items-center gap-2">
          <Icon name="edit" size={15} />
          <span className="text-[13px] font-semibold">{t('home.editMessage')}</span>
          {message.role === 'user' && (
            <span className="text-[11px] text-[var(--text-muted)]">{t('home.editRerunHint')}</span>
          )}
        </div>
        <textarea
          ref={inputRef}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          rows={6}
          className="mt-3 w-full resize-y rounded-xl modern-card border-[var(--border)] focus:border-[var(--accent)] outline-none p-3 text-[13px] leading-relaxed"
          placeholder={t('home.editMessagePlaceholder')}
        />
        <div className="flex items-center justify-end gap-2 mt-4">
          <button
            onClick={onCancel}
            className="h-8 px-4 rounded-lg border border-[var(--border)] text-[12px] font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {t('home.cancel')}
          </button>
          <button
            onClick={() => onSubmit(value.trim())}
            disabled={!value.trim()}
            className="h-8 px-4 rounded-lg btn-primary text-[12px] font-medium transition-all disabled:opacity-50"
          >
            {t('home.editMessageSave')}
          </button>
        </div>
      </div>
    </div>
  )
}

/* ============ Rules 编辑弹窗：全局指令 + 项目级 rules（保存后注入 system_prompt） ============ */
import { RULE_TEMPLATES, type RuleTemplate, type RuleTemplateScope } from '../../data/ruleTemplates'

export function RulesDialog({
  tab,
  setTab,
  globalText,
  setGlobalText,
  projectText,
  setProjectText,
  saving,
  onSave,
  onClose,
}: {
  tab: 'global' | 'project'
  setTab: (t: 'global' | 'project') => void
  globalText: string
  setGlobalText: (v: string) => void
  projectText: string
  setProjectText: (v: string) => void
  saving: boolean
  onSave: () => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  // 应用模板浮层：null = 关闭；string = 待确认的模板 id
  const [pendingTemplate, setPendingTemplate] = useState<RuleTemplate | null>(null)

  // 当前 tab 的可套用模板（按 category 过滤）
  const availableTemplates = useMemo(() => {
    return RULE_TEMPLATES.filter((tp) => tp.scope === 'both' || tp.scope === (tab as RuleTemplateScope))
  }, [tab])
  // 按 category 分组
  const groupedTemplates = useMemo(() => {
    const map = new Map<string, RuleTemplate[]>()
    for (const tp of availableTemplates) {
      const arr = map.get(tp.category) ?? []
      arr.push(tp)
      map.set(tp.category, arr)
    }
    return Array.from(map.entries())
  }, [availableTemplates])

  // 应用模板：替换 or 追加
  const applyTemplate = (tp: RuleTemplate, mode: 'append' | 'replace') => {
    const setter = tab === 'global' ? setGlobalText : setProjectText
    const current = tab === 'global' ? globalText : projectText
    if (mode === 'replace') {
      setter(tp.content)
    } else {
      // 追加：已有内容则加换行分隔
      const sep = current.trim() ? '\n\n' : ''
      setter(current + sep + tp.content)
    }
    setPendingTemplate(null)
  }

  return (
    <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
      <div className="w-[620px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
        <div className="flex items-center gap-2">
          <Icon name="settings" size={15} />
          <span className="text-[13px] font-semibold">{t('home.rules')}</span>
          <span className="text-[11px] text-[var(--text-muted)]">{t('home.rulesHint')}</span>
        </div>
        {/* Tab：全局指令 / 项目级 */}
        <div className="flex items-center gap-1 mt-3 bg-[var(--bg-card)] rounded-lg p-1 w-fit">
          <button
            onClick={() => setTab('global')}
            className={`h-7 px-3 rounded-md text-[12px] font-medium transition-colors ${
              tab === 'global' ? 'tab-active' : 'tab-inactive'
            }`}
          >
            {t('home.rulesGlobal')}
          </button>
          <button
            onClick={() => setTab('project')}
            className={`h-7 px-3 rounded-md text-[12px] font-medium transition-colors ${
              tab === 'project' ? 'tab-active' : 'tab-inactive'
            }`}
          >
            {t('home.rulesProject')}
          </button>
        </div>
        {/* 模板选择下拉：按 category 分组，点击进入确认模式 */}
        <div className="mt-3 modern-card rounded-lg p-2 max-h-[120px] overflow-y-auto">
          <p className="text-[10.5px] font-medium text-[var(--text-muted)] px-1.5 py-1 uppercase tracking-wider">
            {t('home.ruleTemplate')}
          </p>
          {groupedTemplates.map(([cat, items]) => (
            <div key={cat} className="mt-1">
              <p className="text-[10px] text-[var(--text-muted)] px-1.5 py-0.5">
                {t(`home.ruleTemplateCategory.${cat}`)}
              </p>
              <div className="flex flex-wrap gap-1 px-1">
                {items.map((tp) => (
                  <button
                    key={tp.id}
                    onClick={() => setPendingTemplate(tp)}
                    title={t(`${tp.i18nKey}.desc`)}
                    className="px-2 py-0.5 rounded-md text-[11px] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:bg-[var(--accent-soft)] hover:text-[var(--accent)] border border-[var(--border)] transition-colors"
                  >
                    {t(tp.i18nKey)}
                  </button>
                ))}
              </div>
            </div>
          ))}
        </div>
        <textarea
          value={tab === 'global' ? globalText : projectText}
          onChange={(e) => (tab === 'global' ? setGlobalText(e.target.value) : setProjectText(e.target.value))}
          rows={14}
          className="mt-3 w-full resize-y rounded-xl modern-card border-[var(--border)] focus:border-[var(--accent)] outline-none p-3 text-[13px] leading-relaxed font-mono"
          placeholder={
            tab === 'global'
              ? t('home.rulesGlobalPlaceholder')
              : t('home.rulesProjectPlaceholder', { name: '项目' })
          }
        />
        <div className="flex items-center justify-between mt-4">
          <span className="text-[11px] text-[var(--text-muted)]">{t('home.rulesInjectHint')}</span>
          <div className="flex items-center gap-2">
            <button
              onClick={onClose}
              className="h-8 px-4 rounded-lg border border-[var(--border)] text-[12px] font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              {t('home.cancel')}
            </button>
            <button
              onClick={onSave}
              disabled={saving}
              className="h-8 px-4 rounded-lg btn-primary text-[12px] font-medium transition-all disabled:opacity-50"
            >
              {saving ? t('home.rulesSaving') : t('home.rulesSave')}
            </button>
          </div>
        </div>
      </div>

      {/* 模板套用确认浮层：避免点一下就覆盖用户的现有内容 */}
      {pendingTemplate && (
        <div className="cmdk-backdrop" onClick={() => setPendingTemplate(null)}>
          <div
            className="w-[420px] max-w-[90vw] rounded-2xl glass-card p-4 animate-modal-in"
            onClick={(e) => e.stopPropagation()}
          >
            <p className="text-[13px] font-semibold mb-1">{t('home.ruleTemplateConfirmTitle')}</p>
            <p className="text-[12px] text-[var(--text-secondary)] mb-3">
              {t(pendingTemplate.i18nKey)} — {t('home.ruleTemplateConfirmBody')}
            </p>
            <div className="flex justify-end gap-2">
              <button
                onClick={() => setPendingTemplate(null)}
                className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] hover:bg-[var(--bg-hover)] transition-colors"
              >
                {t('home.cancel')}
              </button>
              <button
                onClick={() => applyTemplate(pendingTemplate, 'append')}
                className="h-8 px-3 rounded-lg btn-ghost text-[12px] transition-colors"
              >
                {t('home.ruleTemplateConfirmAppend')}
              </button>
              <button
                onClick={() => applyTemplate(pendingTemplate, 'replace')}
                className="h-8 px-3 rounded-lg btn-primary text-[12px] transition-all"
              >
                {t('home.ruleTemplateConfirmReplace')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}




