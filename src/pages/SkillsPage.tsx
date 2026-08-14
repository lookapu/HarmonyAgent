import { useState, useEffect, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { listSkills, importSkillFromGithub, toggleSkill, removeSkill, cloneSkill, type Skill } from '../api/skill'
import { skillTemplates, type SkillTemplate } from '../data/skillTemplates'
import { useProjectStore } from '../stores/projectStore'
import Icon from '../icons/Icon'

/** 从 GitHub 地址（URL / git@ / owner/name）提取 owner 和 name */
function parseGithubUrl(input: string): { owner: string; name: string } | null {
  const s = input.trim().replace(/\/+$/, '')
  if (!s) return null
  let rest = s
    .replace(/^https?:\/\/github\.com\//i, '')
    .replace(/^git@github\.com:/, '')
    .replace(/\.git$/i, '')
  rest = rest.split(/[?#]/)[0]
  const parts = rest.split('/').filter(Boolean)
  if (parts.length >= 2) return { owner: parts[0], name: parts[1] }
  return null
}

export default function SkillsPage() {
  const { t } = useTranslation()
  const currentProject = useProjectStore((s) => s.currentProject)
  // 全局项目（id='global'）也支持项目级专属技能
  const projectId = currentProject ? currentProject.id : null
  const [scope, setScope] = useState<'global' | 'project'>('global')
  const effectiveScope: 'global' | 'project' = projectId ? scope : 'global'
  const [skills, setSkills] = useState<Skill[]>([])
  const [showForm, setShowForm] = useState(false)
  const [repoUrl, setRepoUrl] = useState('')
  const [branch, setBranch] = useState('')
  const [importing, setImporting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [parsed, setParsed] = useState<{ owner: string; name: string } | null>(null)
  const [useProxy, setUseProxy] = useState(false)
  const [installingKey, setInstallingKey] = useState<string | null>(null)

  const load = async () => {
    try {
      const list = await listSkills(projectId)
      setSkills(list)
    } catch (e) {
      console.error(e)
    }
  }

  useEffect(() => { load() }, [projectId])

  const visibleSkills = useMemo(
    () => skills.filter((s) => effectiveScope === 'global' ? !s.project_id : s.project_id === projectId),
    [skills, effectiveScope, projectId],
  )

  // URL 智能解析：粘贴地址后自动提取 owner/name
  useEffect(() => {
    setParsed(parseGithubUrl(repoUrl))
  }, [repoUrl])

  const handleImport = async () => {
    if (!parsed || importing) return
    setError(null)
    setImporting(true)
    try {
      await importSkillFromGithub(repoUrl, branch.trim() || undefined, useProxy, undefined, effectiveScope === 'project' ? projectId : null)
      setRepoUrl('')
      setBranch('')
      setShowForm(false)
      load()
    } catch (e) {
      setError(String(e))
    } finally {
      setImporting(false)
    }
  }

  const handleToggle = async (id: string, enabled: boolean) => {
    await toggleSkill(id, !enabled)
    load()
  }

  const handleRemove = async (id: string, name: string) => {
    if (!window.confirm(t('skill.deleteConfirm', { name }))) return
    try {
      await removeSkill(id)
      load()
    } catch (e) {
      alert(t('skill.deleteFailed', { err: String(e) }))
    }
  }

  const handleClone = async (s: Skill) => {
    const target = effectiveScope === 'global' ? projectId : null
    const targetLabel = target ? t('common.scopeProject') : t('common.scopeGlobal')
    if (!window.confirm(t('common.scopeCloneConfirm', { name: s.name, target: targetLabel }))) return
    try {
      await cloneSkill(s.id, target)
      load()
    } catch (e) {
      alert(String(e))
    }
  }

  /** 一键导入内置技能（已安装时后端会自动更新，重复导入安全） */
  const handleImportTemplate = async (tpl: SkillTemplate) => {
    if (installingKey) return
    setInstallingKey(tpl.key)
    setError(null)
    try {
      await importSkillFromGithub(tpl.repo, tpl.branch, useProxy, tpl.subdir, effectiveScope === 'project' ? projectId : null)
      load()
    } catch (e) {
      setError(String(e))
    } finally {
      setInstallingKey(null)
    }
  }

  // 已安装集合（按 repo + subdir 匹配）
  const installedKeys = new Set(
    skills
      .filter((s) => s.repo_owner && s.repo_name)
      .map((s) => `${s.repo_owner}/${s.repo_name}/${s.subdir || ''}`),
  )

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-xl font-semibold">{t('skill.title')}</h2>
          <p className="text-xs text-[var(--text-secondary)] mt-1">{t('skill.importTitle')}</p>
        </div>
        <button
          onClick={() => setShowForm(!showForm)}
          className="h-9 px-4 rounded-[10px] bg-[var(--accent)] text-white text-[13px] font-medium hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all shadow-lg shadow-[var(--accent)]/15"
        >
          <span className="flex items-center gap-1.5">
            <Icon name={showForm ? 'close' : 'download'} size={14} white />
            {showForm ? t('mcp.cancel') : t('skill.installHint')}
          </span>
        </button>
      </div>

      {projectId && (
        <div className="inline-flex bg-[var(--bg-card)] border border-[var(--border)] rounded-lg p-0.5 mb-4 text-[12px]">
          <button
            onClick={() => setScope('global')}
            className={`px-3 h-7 rounded-md transition-colors ${scope === 'global' ? 'bg-[var(--accent)] text-white' : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'}`}
          >
            {t('common.scopeGlobal')}
          </button>
          <button
            onClick={() => setScope('project')}
            className={`px-3 h-7 rounded-md transition-colors ${scope === 'project' ? 'bg-[var(--accent)] text-white' : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'}`}
          >
            {t('common.scopeProject')}
          </button>
        </div>
      )}
      {!projectId && (
        <p className="text-[11px] text-[var(--text-muted)] mb-4">{t('common.scopeGlobalOnly')}</p>
      )}
      {projectId && effectiveScope === 'project' && !currentProject?.trusted && (
        <div className="mb-4 px-3 py-2 rounded-lg border border-[var(--warning)]/40 bg-[var(--warning)]/10 text-[11px] text-[var(--warning)]">
          ⚠️ {t('common.untrustedScopeWarn')}
        </div>
      )}

      {showForm && (
        <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-2xl p-4 mb-6 space-y-3 animate-fade-in-up">
          <div className="space-y-1.5">
            <label className="text-[11px] text-[var(--text-muted)]">{t('skill.repoUrl')}</label>
            <input
              placeholder={t('skill.repoUrlPlaceholder')}
              value={repoUrl}
              onChange={(e) => setRepoUrl(e.target.value)}
              className="w-full h-9 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
            />
            {parsed && (
              <p className="text-[11px] text-[var(--success)] font-mono">
                ✓ {parsed.owner}/{parsed.name}
              </p>
            )}
          </div>
          <div className="space-y-1.5">
            <label className="text-[11px] text-[var(--text-muted)]">{t('skill.branch')}</label>
            <input
              placeholder="main"
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
              className="w-full h-9 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
            />
          </div>
          {/* 走系统代理克隆（国内网络访问 GitHub 时常用） */}
          <label className="flex items-center gap-2 cursor-pointer select-none">
            <input
              type="checkbox"
              checked={useProxy}
              onChange={(e) => setUseProxy(e.target.checked)}
              className="w-3.5 h-3.5 accent-[var(--accent)]"
            />
            <span className="text-[12px] text-[var(--text-secondary)]">{t('skill.useProxy')}</span>
          </label>
          <div className="flex items-center gap-3">
            {error && <span className="text-xs text-[var(--danger)] break-all flex-1">{error}</span>}
            <button
              onClick={handleImport}
              disabled={!parsed || importing}
              className="h-9 px-5 rounded-[10px] bg-[var(--success)] text-white text-[13px] font-medium hover:opacity-90 active:scale-[0.98] transition-all disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {importing ? t('skill.importing') : t('skill.import')}
            </button>
          </div>
        </div>
      )}

      {/* 内置技能库：GitHub 热门精选，一键导入 */}
      <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-2xl p-4 mb-6">
        <div className="flex items-start justify-between gap-3">
          <div>
            <div className="text-[11px] font-medium text-[var(--text-muted)]">{t('skill.builtinTitle')}</div>
            <p className="text-[10px] text-[var(--text-muted)] mt-0.5 mb-3">{t('skill.builtinHint')}</p>
          </div>
          {/* 内置技能安装同样支持走系统代理（国内网络访问 GitHub 时常用） */}
          <label className="flex items-center gap-1.5 cursor-pointer select-none shrink-0 pt-0.5">
            <input
              type="checkbox"
              checked={useProxy}
              onChange={(e) => setUseProxy(e.target.checked)}
              className="w-3.5 h-3.5 accent-[var(--accent)]"
            />
            <span className="text-[12px] text-[var(--text-secondary)] whitespace-nowrap">{t('skill.useProxy')}</span>
          </label>
        </div>
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-2">
          {skillTemplates.map((tpl) => {
            const installed = installedKeys.has(`${tpl.repo}/${tpl.subdir || ''}`)
            const importing = installingKey === tpl.key
            const rec = tpl.recommended
            return (
              <button
                key={tpl.key}
                onClick={() => handleImportTemplate(tpl)}
                disabled={installingKey !== null}
                className={`text-left p-2.5 rounded-lg border transition-all ${
                  installed
                    ? 'border-[var(--success)]/50 bg-[var(--success)]/10'
                    : 'border-[var(--border)] bg-[var(--bg-card)] hover:border-[var(--accent)]/40 hover:bg-[var(--bg-hover)]'
                } disabled:opacity-50 disabled:cursor-not-allowed`}
                title={`${tpl.description}\n${tpl.popularity ? t('skill.popularity', { data: tpl.popularity }) : ''}\n${tpl.badge}\n${t('skill.clickToImport')}`}
              >
                <span
                  className={`block text-[12px] font-medium truncate ${
                    installed ? 'text-[var(--success)]' : 'text-[var(--text-primary)]'
                  }`}
                >
                  {importing ? t('skill.installing') : installed ? `✓ ${t('skill.installed')}` : tpl.name}
                  {rec === 'hot' && !installed && !importing && (
                    <span className="ml-1 px-1 py-px rounded bg-[#f59e0b]/15 text-[#f59e0b] text-[9px] font-bold align-middle">🔥 {t('skill.hot')}</span>
                  )}
                  {rec === 'popular' && !installed && !importing && (
                    <span className="ml-1 px-1 py-px rounded bg-[var(--accent)]/10 text-[var(--accent)] text-[9px] font-bold align-middle">⭐ {t('skill.popular')}</span>
                  )}
                </span>
                <span className="block text-[11px] text-[var(--text-secondary)] mt-0.5 leading-snug">{tpl.description}</span>
                <span className="block text-[10px] text-[var(--text-muted)] mt-1 font-mono truncate">{tpl.badge}</span>
              </button>
            )
          })}
        </div>
        <p className="text-[10px] text-[var(--text-muted)] mt-2">{t('skill.recommendedTip')}</p>
      </div>

      <div className="space-y-3">
        {visibleSkills.length === 0 && (
          <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-8 text-center">
            <p className="text-[var(--text-secondary)] text-sm">{t('skill.empty')}</p>
            <p className="text-xs text-[var(--text-secondary)] mt-2">
              {t('skill.importTitle')}：{t('skill.importHint')}
            </p>
          </div>
        )}
        {visibleSkills.map((s) => (
          <div key={s.id} className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4 flex items-center justify-between">
            <div>
              <div className="flex items-center gap-2">
                <span className="font-medium">{s.name}</span>
                <span className={`text-xs px-2 py-0.5 rounded ${s.enabled ? 'bg-[var(--success)] text-white' : 'bg-[var(--bg-card)] text-[var(--text-secondary)]'}`}>
                  {s.enabled ? t('skill.enabled') : t('skill.disabled')}
                </span>
              </div>
              {s.description && <p className="text-xs text-[var(--text-secondary)] mt-1">{s.description}</p>}
              {s.repo_owner && (
                <p className="text-xs text-[var(--text-secondary)] mt-1 font-mono">
                  {s.repo_owner}/{s.repo_name} ({s.repo_branch})
                </p>
              )}
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <button
                onClick={() => handleToggle(s.id, s.enabled)}
                className="px-3 py-1 text-xs border border-[var(--border)] rounded hover:bg-[var(--bg-card)] transition-colors"
              >
                {s.enabled ? t('skill.disable') : t('skill.enable')}
              </button>
              {projectId && (
                <button
                  onClick={() => handleClone(s)}
                  title={effectiveScope === 'global' ? t('common.scopeCloneToProject') : t('common.scopeCloneToGlobal')}
                  className="px-3 py-1 text-xs border border-[var(--border)] rounded hover:bg-[var(--bg-card)] transition-colors"
                >
                  {effectiveScope === 'global' ? t('common.scopeCloneToProject') : t('common.scopeCloneToGlobal')}
                </button>
              )}
              <button
                onClick={() => handleRemove(s.id, s.name)}
                title={t('skill.deleteTitle')}
                className="px-3 py-1 text-xs border border-[var(--danger)]/40 text-[var(--danger)] rounded hover:bg-[var(--danger)]/10 transition-colors"
              >
                <Icon name="delete" size={12} />
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
