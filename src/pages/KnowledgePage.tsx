// @ui-states: loading, empty, failed
import { useState, useEffect, useCallback } from 'react'
import { useTranslation } from 'react-i18next'
import Icon from '../icons/Icon'
import { useProjectStore } from '../stores/projectStore'
import {
  listKnowledge,
  addKnowledge,
  updateKnowledge,
  toggleKnowledge,
  deleteKnowledge,
  cloneKnowledge,
  type KnowledgeEntry,
} from '../api/knowledge'

const emptyForm = { keywords: '', title: '', cause: '', fix: '', enabled: true }

export default function KnowledgePage() {
  const { t } = useTranslation()
  const currentProject = useProjectStore((s) => s.currentProject)
  const projectId = currentProject ? currentProject.id : null
  const [scope, setScope] = useState<'global' | 'project'>('global')
  const effectiveScope: 'global' | 'project' = projectId ? scope : 'global'

  const [entries, setEntries] = useState<KnowledgeEntry[]>([])
  const [showForm, setShowForm] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [form, setForm] = useState(emptyForm)
  const [saving, setSaving] = useState(false)

  const scopeProjectId = effectiveScope === 'project' ? projectId : null

  // useCallback 稳定引用：scope 变化时 load 重建触发 effect，避免每次渲染重复加载
  const load = useCallback(async () => {
    try {
      setEntries(await listKnowledge(scopeProjectId))
    } catch (e) {
      console.error(e)
    }
  }, [scopeProjectId])
  useEffect(() => { load() }, [load])

  const resetForm = () => {
    setForm(emptyForm)
    setEditingId(null)
    setShowForm(false)
  }

  const handleSave = async () => {
    if (!form.keywords.trim() || !form.title.trim()) {
      alert(t('knowledge.fieldsRequired'))
      return
    }
    setSaving(true)
    try {
      if (editingId) {
        await updateKnowledge(editingId, form, scopeProjectId)
      } else {
        await addKnowledge(form, scopeProjectId)
      }
      resetForm()
      load()
    } catch (e) {
      alert(String(e))
    } finally {
      setSaving(false)
    }
  }

  const startEdit = (e: KnowledgeEntry) => {
    setEditingId(e.id)
    setForm({ keywords: e.keywords, title: e.title, cause: e.cause, fix: e.fix, enabled: e.enabled })
    setShowForm(true)
  }

  const handleToggle = (id: string, enabled: boolean) =>
    toggleKnowledge(id, enabled, scopeProjectId).then(load).catch((e) => alert(String(e)))

  const handleDelete = (e: KnowledgeEntry) => {
    if (e.builtin) return
    if (!window.confirm(t('knowledge.deleteConfirm', { name: e.title }))) return
    deleteKnowledge(e.id).then(load).catch((err) => alert(String(err)))
  }

  const handleClone = async (e: KnowledgeEntry) => {
    const target = effectiveScope === 'global' ? projectId : null
    const targetLabel = target ? t('common.scopeProject') : t('common.scopeGlobal')
    if (!confirm(t('common.scopeCloneConfirm', { name: e.title, target: targetLabel }))) return
    try {
      await cloneKnowledge(e.id, target)
      load()
    } catch (err) {
      alert(String(err))
    }
  }

  return (
    <div className="space-y-5">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold">{t('knowledge.title')}</h2>
          <p className="text-xs text-[var(--text-secondary)] mt-1">{t('knowledge.desc')}</p>
        </div>
        <button
          onClick={() => { resetForm(); setShowForm(!showForm) }}
          className="h-9 px-4 rounded-[10px] btn-primary text-[13px] font-medium transition-all"
        >
          <span className="flex items-center gap-1.5">
            <Icon name={showForm ? 'close' : 'plus'} size={14} white />
            {showForm ? t('mcp.cancel') : t('knowledge.add')}
          </span>
        </button>
      </div>

      {projectId && (
        <div className="inline-flex p-0.5 rounded-lg bg-[var(--bg-secondary)]">
          <button
            onClick={() => setScope('global')}
            className={`h-7 px-3 rounded-md text-[12px] transition-all ${
              effectiveScope === 'global'
                ? 'bg-[var(--bg-card)] text-[var(--text-primary)] shadow-sm'
                : 'tab-inactive'
            }`}
          >
            {t('common.scopeGlobal')}
          </button>
          <button
            onClick={() => setScope('project')}
            className={`h-7 px-3 rounded-md text-[12px] transition-all ${
              effectiveScope === 'project'
                ? 'bg-[var(--bg-card)] text-[var(--text-primary)] shadow-sm'
                : 'tab-inactive'
            }`}
          >
            {t('common.scopeProject')}
          </button>
        </div>
      )}
      {!projectId && <p className="text-[11px] text-[var(--text-muted)]">{t('common.scopeGlobalOnly')}</p>}
      {projectId && effectiveScope === 'project' && !currentProject?.trusted && (
        <div className="px-3 py-2 rounded-lg border border-[var(--warning)]/40 bg-[var(--warning)]/10 text-[11px] text-[var(--warning)]">
          ⚠️ {t('common.untrustedScopeWarn')}
        </div>
      )}

      {showForm && (
        <div className="modern-card rounded-xl p-4 space-y-3">
          <div>
            <label className="block text-xs text-[var(--text-secondary)] mb-1">{t('knowledge.keywords')}</label>
            <input
              value={form.keywords}
              onChange={(e) => setForm({ ...form, keywords: e.target.value })}
              placeholder={t('knowledge.keywordsHint')}
              className="w-full h-9 px-3 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[13px] outline-none focus:border-[var(--accent)]"
            />
          </div>
          <div>
            <label className="block text-xs text-[var(--text-secondary)] mb-1">{t('knowledge.entryTitle')}</label>
            <input
              value={form.title}
              onChange={(e) => setForm({ ...form, title: e.target.value })}
              className="w-full h-9 px-3 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[13px] outline-none focus:border-[var(--accent)]"
            />
          </div>
          <div>
            <label className="block text-xs text-[var(--text-secondary)] mb-1">{t('knowledge.cause')}</label>
            <textarea
              value={form.cause}
              onChange={(e) => setForm({ ...form, cause: e.target.value })}
              rows={2}
              className="w-full px-3 py-2 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[13px] outline-none focus:border-[var(--accent)] resize-y"
            />
          </div>
          <div>
            <label className="block text-xs text-[var(--text-secondary)] mb-1">{t('knowledge.fix')}</label>
            <textarea
              value={form.fix}
              onChange={(e) => setForm({ ...form, fix: e.target.value })}
              rows={3}
              className="w-full px-3 py-2 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[13px] outline-none focus:border-[var(--accent)] resize-y"
            />
          </div>
          <label className="flex items-center gap-2 text-[13px] cursor-pointer">
            <input type="checkbox" checked={form.enabled} onChange={(e) => setForm({ ...form, enabled: e.target.checked })} />
            {t('knowledge.enabled')}
          </label>
          <button
            onClick={handleSave}
            disabled={saving}
            className="h-9 px-4 rounded-lg btn-primary text-[13px]  disabled:opacity-50"
          >
            {editingId ? t('knowledge.saveEdit') : t('knowledge.save')}
          </button>
        </div>
      )}

      <div className="space-y-2">
        {entries.length === 0 && (
          <div className="modern-card rounded-lg p-8 text-center text-sm text-[var(--text-secondary)]">
            {t('knowledge.empty')}
          </div>
        )}
        {entries.map((e) => (
          <div key={e.id} className="modern-card rounded-xl p-4">
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 flex-wrap">
                  <h3 className="font-medium text-[14px] truncate">{e.title}</h3>
                  {e.builtin && (
                    <span className="px-1.5 h-[18px] inline-flex items-center rounded bg-[var(--accent-soft)] text-[var(--accent)] text-[10px]">
                      {t('knowledge.builtin')}
                    </span>
                  )}
                  {!e.enabled && (
                    <span className="px-1.5 h-[18px] inline-flex items-center rounded bg-[var(--bg-hover)] text-[var(--text-muted)] text-[10px]">
                      {t('knowledge.disabled')}
                    </span>
                  )}
                </div>
                <div className="text-[11px] text-[var(--text-muted)] mt-1 break-words">
                  {t('knowledge.keywords')}: {e.keywords}
                  {e.hit_count > 0 && (
                    <span className="ml-2 text-[var(--accent)]">· {t('knowledge.hitCount', { n: e.hit_count })}</span>
                  )}
                </div>
                {e.cause && <p className="text-[12px] text-[var(--text-secondary)] mt-2 whitespace-pre-wrap">{e.cause}</p>}
                {e.fix && (
                  <p className="text-[12px] text-[var(--success)] mt-1 whitespace-pre-wrap">
                    <span className="text-[var(--text-muted)]">{t('knowledge.fix')}: </span>{e.fix}
                  </p>
                )}
              </div>
              <div className="flex gap-1.5 shrink-0 flex-wrap justify-end">
                <button
                  onClick={() => handleToggle(e.id, !e.enabled)}
                  className="px-2.5 h-7 rounded-md border border-[var(--border)] text-[11px] hover:bg-[var(--bg-hover)]"
                >
                  {e.enabled ? t('mcp.disable') : t('mcp.enable')}
                </button>
                {projectId && (
                  <button
                    onClick={() => handleClone(e)}
                    title={effectiveScope === 'global' ? t('common.scopeCloneToProject') : t('common.scopeCloneToGlobal')}
                    className="px-2.5 h-7 rounded-md border border-[var(--border)] text-[11px] hover:bg-[var(--bg-hover)]"
                  >
                    {effectiveScope === 'global' ? t('common.scopeCloneToProject') : t('common.scopeCloneToGlobal')}
                  </button>
                )}
                {!e.builtin && (
                  <>
                    <button
                      onClick={() => startEdit(e)}
                      className="px-2.5 h-7 rounded-md border border-[var(--border)] text-[11px] hover:bg-[var(--bg-hover)]"
                    >
                      {t('knowledge.edit')}
                    </button>
                    <button
                      onClick={() => handleDelete(e)}
                      className="px-2.5 h-7 rounded-md border border-[var(--danger)]/40 text-[var(--danger)] text-[11px] hover:bg-[var(--danger)]/10"
                    >
                      {t('knowledge.delete')}
                    </button>
                  </>
                )}
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}





