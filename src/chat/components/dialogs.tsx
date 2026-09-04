import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useProjectStore } from '../../stores/projectStore'
import type { MessageVersion, ChatMessage, MemoryDraft } from '../../api/project'
import { diffWords, type Change } from 'diff'
import { ThumbDownIcon } from './messageBlocks'
import { Modal } from '../../components/ui/Modal'
import { Button } from '../../components/ui/Button'

/* ============ 点踩反馈弹窗（可选原因） ============ */
export function FeedbackDialog({ onSubmit, onCancel }: { onSubmit: (reason?: string) => void; onCancel: () => void }) {
  const { t } = useTranslation()
  const [reason, setReason] = useState('')
  const reasons = ['内容不准确', '代码有错误', '没有帮助', '语气不合适', '其他']
  return (
    <Modal
      open
      onClose={onCancel}
      backdropClose={false}
      size="sm"
      title={
        <span className="inline-flex min-w-0 items-center gap-2">
          <span aria-hidden="true" className="inline-flex shrink-0">
            <ThumbDownIcon filled />
          </span>
          {t('home.feedbackTitle')}
        </span>
      }
      footer={
        <>
          <Button variant="secondary" size="md" onClick={onCancel}>
            {t('home.cancel')}
          </Button>
          <Button variant="primary" size="md" onClick={() => onSubmit(reason || undefined)}>
            {t('home.feedbackSubmit')}
          </Button>
        </>
      }
    >
      <div className="flex flex-wrap gap-1.5">
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
    </Modal>
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
      <Modal open onClose={onClose} size="lg">
        <p className="text-[12px] text-[var(--text-muted)]">{t('home.noVersions')}</p>
        <Button variant="primary" size="md" className="mt-3" onClick={onClose}>
          {t('home.close')}
        </Button>
      </Modal>
    )
  }
  // diff：当前回复 vs 旧版本（旧 → 新 方向，添加为绿、删除为红）
  const changes: Change[] = diffWords(selected.content, current)
  return (
    <Modal
      open
      onClose={onClose}
      size="2xl"
      maxHeight="80vh"
      icon="git-branch"
      title={t('home.versionCompare')}
    >
      {/* h-full + 内层自己滚：Modal 的 body 是 overflow-y-auto，若不约束高度，
          版本选择器与图例会跟着 diff 一起滚走，长回复对比时就找不到切换入口了。 */}
      <div className="flex h-full flex-col">
        {/* 版本选择器（按时间倒序：最新旧版在前） */}
        <div className="flex shrink-0 flex-wrap items-center gap-1.5">
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
        <div className="mt-3 min-h-0 flex-1 overflow-y-auto rounded-xl modern-card border-[var(--border)] p-3 text-[12px] leading-relaxed whitespace-pre-wrap break-words">
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
        <div className="mt-3 flex shrink-0 items-center gap-3 text-[10px] text-[var(--text-muted)]">
          <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-sm bg-[var(--success)]/50" />{t('home.diffAdded')}</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 rounded-sm bg-[var(--danger)]/50" />{t('home.diffRemoved')}</span>
          <span className="ml-auto">{t('home.diffHint')}</span>
        </div>
      </div>
    </Modal>
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
    <Modal
      open
      onClose={onCancel}
      size="lg"
      icon="lightbulb"
      title={t('home.memoryDraftTitle')}
      footer={
        <>
          <Button variant="secondary" size="md" onClick={onCancel}>
            {t('home.cancel')}
          </Button>
          <Button variant="primary" size="md" onClick={onConfirm}>
            {t('home.memoryDraftSave')}
          </Button>
        </>
      }
    >
      <div className="space-y-2.5">
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
    </Modal>
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
    <Modal
      open
      onClose={onCancel}
      backdropClose={false}
      size="lg"
      icon="edit"
      title={
        <>
          {t('home.editMessage')}
          {message.role === 'user' && (
            <span className="ml-2 font-normal text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
              {t('home.editRerunHint')}
            </span>
          )}
        </>
      }
      footer={
        <>
          <Button variant="secondary" size="md" onClick={onCancel}>
            {t('home.cancel')}
          </Button>
          <Button
            variant="primary"
            size="md"
            onClick={() => onSubmit(value.trim())}
            disabled={!value.trim()}
          >
            {t('home.editMessageSave')}
          </Button>
        </>
      }
    >
      <textarea
        ref={inputRef}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        rows={6}
        className="w-full resize-y rounded-xl modern-card border-[var(--border)] focus:border-[var(--accent)] outline-none p-3 text-[13px] leading-relaxed"
        placeholder={t('home.editMessagePlaceholder')}
      />
    </Modal>
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
    <>
      <Modal
        open
        onClose={onClose}
        backdropClose={false}
        size="xl"
        icon="settings"
        title={
          <>
            {t('home.rules')}
            <span className="ml-2 font-normal text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
              {t('home.rulesHint')}
            </span>
          </>
        }
      >
        {/* Tab：全局指令 / 项目级 */}
        <div className="flex items-center gap-1 bg-[var(--bg-card)] rounded-lg p-1 w-fit">
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
                    // tp.i18nKey 本身不带 home. 前缀，但译文是平铺在 home 对象里的
                    // 带点键（home."ruleTemplate.xxx"），漏前缀会直接把 key 渲染给用户
                    title={t(`home.${tp.i18nKey}.desc`)}
                    className="px-2 py-0.5 rounded-md text-[11px] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:bg-[var(--accent-soft)] hover:text-[var(--accent)] border border-[var(--border)] transition-colors"
                  >
                    {t(`home.${tp.i18nKey}`)}
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
        {/* 底部行是「左提示 + 右按钮」，不是 Modal footer 的纯右对齐，故留在正文里 */}
        <div className="mt-3 flex items-center justify-between gap-3">
          <span className="text-[11px] text-[var(--text-muted)]">{t('home.rulesInjectHint')}</span>
          <div className="flex shrink-0 items-center gap-2">
            <Button variant="secondary" size="md" onClick={onClose}>
              {t('home.cancel')}
            </Button>
            <Button variant="primary" size="md" loading={saving} onClick={onSave}>
              {saving ? t('home.rulesSaving') : t('home.rulesSave')}
            </Button>
          </div>
        </div>
      </Modal>

      {/* 模板套用确认浮层：避免点一下就覆盖用户的现有内容。
          与 RulesDialog 是嵌套模态——两者都注册 Esc，useEscapeKey 的模块级栈
          保证 Esc 只关最上层这一个，不会把外层 Rules 一起关掉。 */}
      {pendingTemplate && (
        <Modal
          open
          onClose={() => setPendingTemplate(null)}
          size="sm"
          align="top"
          maxHeight="none"
          title={t('home.ruleTemplateConfirmTitle')}
          footer={
            <>
              <Button variant="secondary" size="md" onClick={() => setPendingTemplate(null)}>
                {t('home.cancel')}
              </Button>
              <Button
                variant="secondary"
                size="md"
                onClick={() => applyTemplate(pendingTemplate, 'append')}
              >
                {t('home.ruleTemplateConfirmAppend')}
              </Button>
              <Button
                variant="primary"
                size="md"
                onClick={() => applyTemplate(pendingTemplate, 'replace')}
              >
                {t('home.ruleTemplateConfirmReplace')}
              </Button>
            </>
          }
        >
          <p className="text-[12px] text-[var(--text-secondary)]">
            {t(`home.${pendingTemplate.i18nKey}`)} — {t('home.ruleTemplateConfirmBody')}
          </p>
        </Modal>
      )}
    </>
  )
}
