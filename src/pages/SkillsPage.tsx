// @ui-states: loading, empty, failed, retry, permission
import { useState, useEffect, useCallback, useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import {
  listSkills,
  importSkillFromGithub,
  toggleSkill,
  removeSkill,
  cloneSkill,
  listSkillUsage,
  listSkillUsageEvents,
  type Skill,
  type SkillUsageEvent,
  type SkillUsageStat,
} from '../api/skill'
import { skillTemplates, type SkillTemplate } from '../data/skillTemplates'
import { useProjectStore } from '../stores/projectStore'
import Icon from '../icons/Icon'
import { listExtensionGovernance, type ExtensionGovernanceRecord } from '../api/governance'

/** 从 Git 仓库地址（GitHub/Gitee 的 URL / git@ / owner/name）提取 owner 和 name */
function parseGithubUrl(input: string): { owner: string; name: string } | null {
  const s = input.trim().replace(/\/+$/, '')
  if (!s) return null
  let rest = s
    .replace(/^https?:\/\/(github|gitee)\.com\//i, '')
    .replace(/^git@(github|gitee)\.com:/, '')
    .replace(/\.git$/i, '')
  rest = rest.split(/[?#]/)[0]
  const parts = rest.split('/').filter(Boolean)
  if (parts.length >= 2) return { owner: parts[0], name: parts[1] }
  return null
}

/** unix 秒 → 本地日期 + 时分 */
function fmtTime(t: number | null): string {
  if (!t) return '—'
  const d = new Date(t * 1000)
  return `${d.toLocaleDateString()} ${d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`
}

export default function SkillsPage() {
  const { t } = useTranslation()
  const currentProject = useProjectStore((s) => s.currentProject)
  // 全局项目（id='global'）也支持项目级专属技能
  const projectId = currentProject ? currentProject.id : null
  const [scope, setScope] = useState<'global' | 'project'>('global')
  const effectiveScope: 'global' | 'project' = projectId ? scope : 'global'
  /** 视图切换：技能列表 / 使用统计 */
  const [view, setView] = useState<'skills' | 'usage'>('skills')
  const [skills, setSkills] = useState<Skill[]>([])
  const [governance, setGovernance] = useState<Record<string, ExtensionGovernanceRecord>>({})
  // skill_id -> 调用统计（use_skill 工具落库）
  const [usageMap, setUsageMap] = useState<Record<string, SkillUsageStat>>({})
  /** 使用统计（按技能聚合 + 最近调用时间线） */
  const [usageStats, setUsageStats] = useState<SkillUsageStat[]>([])
  const [usageEvents, setUsageEvents] = useState<SkillUsageEvent[]>([])
  const [usageLoading, setUsageLoading] = useState(false)
  const [showForm, setShowForm] = useState(false)
  const [repoUrl, setRepoUrl] = useState('')
  const [branch, setBranch] = useState('')
  const [importing, setImporting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [parsed, setParsed] = useState<{ owner: string; name: string } | null>(null)
  const [useProxy, setUseProxy] = useState(false)
  const [installingKey, setInstallingKey] = useState<string | null>(null)

  // useCallback 稳定引用：projectId 变化时 load 重建触发 effect，避免每次渲染重复加载
  const load = useCallback(async () => {
    try {
      // 技能列表与调用统计并行加载，避免串行阻塞
      const [list, usage, governed] = await Promise.all([listSkills(projectId), listSkillUsage(projectId), listExtensionGovernance()])
      setSkills(list)
      setUsageMap(Object.fromEntries(usage.map((u) => [u.skill_id, u])))
      setGovernance(Object.fromEntries(governed.filter((item) => item.extension_kind === 'skill').map((item) => [item.extension_id, item])))
    } catch (e) {
      console.error(e)
    }
  }, [projectId])

  useEffect(() => { load() }, [load])

  // 使用统计：全局视图统计全部项目，项目视图仅当前项目；进入统计视图时才拉取
  const loadUsage = useCallback(async () => {
    const pid = effectiveScope === 'project' ? projectId : null
    setUsageLoading(true)
    try {
      const [usage, evs] = await Promise.all([
        listSkillUsage(pid),
        listSkillUsageEvents(pid, 200),
      ])
      setUsageStats(usage)
      setUsageEvents(evs)
    } catch (e) {
      console.error(e)
      setUsageStats([])
      setUsageEvents([])
    } finally {
      setUsageLoading(false)
    }
  }, [effectiveScope, projectId])

  useEffect(() => {
    if (view === 'usage') void loadUsage()
  }, [view, loadUsage])

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
    try {
      await toggleSkill(id, !enabled)
      load()
    } catch (e) {
      setError(String(e))
    }
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
          <p className="text-xs text-[var(--text-secondary)] mt-1">
            {view === 'usage' ? t('skillStats.desc') : t('skill.importTitle')}
          </p>
        </div>
        {view === 'skills' && (
          <button
            onClick={() => setShowForm(!showForm)}
            className="h-9 px-4 rounded-[10px] btn-primary text-[13px] font-medium transition-all shadow-lg shadow-[var(--accent)]/15"
          >
            <span className="flex items-center gap-1.5">
              <Icon name={showForm ? 'close' : 'download'} size={14} white />
              {showForm ? t('mcp.cancel') : t('skill.installHint')}
            </span>
          </button>
        )}
      </div>

      {/* 视图切换：技能列表 / 使用统计 */}
      <div className="inline-flex modern-card rounded-lg p-0.5 mb-4 text-[12px]">
        <button
          onClick={() => setView('skills')}
          className={`px-3 h-7 rounded-md transition-colors ${view === 'skills' ? 'tab-active' : 'tab-inactive'}`}
        >
          {t('skill.skillsView')}
        </button>
        <button
          onClick={() => setView('usage')}
          className={`px-3 h-7 rounded-md transition-colors ${view === 'usage' ? 'tab-active' : 'tab-inactive'}`}
        >
          {t('skill.usageView')}
        </button>
      </div>

      {view === 'usage' ? (
        <SkillUsageView
          skills={skills}
          stats={usageStats}
          events={usageEvents}
          loading={usageLoading}
          onRefresh={() => void loadUsage()}
          scope={effectiveScope}
        />
      ) : (
        <>
      {projectId && (
        <div className="inline-flex modern-card rounded-lg p-0.5 mb-4 text-[12px]">
          <button
            onClick={() => setScope('global')}
            className={`px-3 h-7 rounded-md transition-colors ${scope === 'global' ? 'tab-active' : 'tab-inactive'}`}
          >
            {t('common.scopeGlobal')}
          </button>
          <button
            onClick={() => setScope('project')}
            className={`px-3 h-7 rounded-md transition-colors ${scope === 'project' ? 'tab-active' : 'tab-inactive'}`}
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
        <div className="modern-card rounded-2xl p-4 mb-6 space-y-3 animate-fade-in-up">
          <div className="space-y-1.5">
            <label className="text-[11px] text-[var(--text-muted)]">{t('skill.repoUrl')}</label>
            <input
              placeholder={t('skill.repoUrlPlaceholder')}
              value={repoUrl}
              onChange={(e) => setRepoUrl(e.target.value)}
              className="w-full h-9 px-3 modern-card rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
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
              className="w-full h-9 px-3 modern-card rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
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
              className="h-9 px-5 rounded-[10px] bg-[var(--success)] text-white text-[13px] font-medium hover:opacity-90 transition-all disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {importing ? t('skill.importing') : t('skill.import')}
            </button>
          </div>
        </div>
      )}

      {/* 内置技能库：GitHub 热门精选，一键导入 */}
      <div className="modern-card rounded-2xl p-4 mb-6">
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
          <div className="modern-card rounded-lg p-8 text-center">
            <p className="text-[var(--text-secondary)] text-sm">{t('skill.empty')}</p>
            <p className="text-xs text-[var(--text-secondary)] mt-2">
              {t('skill.importTitle')}：{t('skill.importHint')}
            </p>
          </div>
        )}
        {visibleSkills.map((s) => (
          <div key={s.id} className="modern-card rounded-lg p-4 flex items-center justify-between">
            <div>
              <div className="flex items-center gap-2">
                <span className="font-medium">{s.name}</span>
                <span className={`text-xs px-2 py-0.5 rounded ${s.enabled ? 'bg-[var(--success)] text-white' : 'bg-[var(--bg-card)] text-[var(--text-secondary)]'}`}>
                  {s.enabled ? t('skill.enabled') : t('skill.disabled')}
                </span>
                <span className="text-[10px] px-2 py-0.5 rounded bg-[var(--bg-card)] text-[var(--text-secondary)] font-mono">
                  v{s.skill_version} · {s.compatibility_status}
                </span>
              </div>
              {s.description && <p className="text-xs text-[var(--text-secondary)] mt-1">{s.description}</p>}
              {s.repo_owner && (
                <p className="text-xs text-[var(--text-secondary)] mt-1 font-mono">
                  {s.repo_owner}/{s.repo_name} ({s.repo_branch})
                </p>
              )}
              {governance[s.id] && (
                <p className={`text-[10px] mt-1 ${governance[s.id].verification_state === 'verified' ? 'text-[var(--success)]' : governance[s.id].verification_state === 'drifted' || governance[s.id].verification_state === 'invalid' ? 'text-[var(--danger)]' : 'text-[var(--text-muted)]'}`}>
                  扩展治理：{governance[s.id].verification_state === 'verified' ? 'Ed25519 签名有效（发布者身份未钉住）' : governance[s.id].verification_state === 'unsigned' ? '未签名' : '已隔离'}
                  {' · '}{governance[s.id].calls_per_minute}/分钟 · 连续失败 {governance[s.id].consecutive_failures}/{governance[s.id].failure_threshold}
                </p>
              )}
              {usageMap[s.id] && usageMap[s.id].call_count > 0 && (
                <p className="text-xs mt-1 text-[var(--accent)]">
                  {t('skill.calledTimes', { n: usageMap[s.id].call_count })} · {t('skill.lastCalled')}:{' '}
                  {fmtTime(usageMap[s.id].last_called_at)}
                </p>
              )}
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <button
                onClick={() => handleToggle(s.id, s.enabled)}
                disabled={!s.enabled && s.compatibility_status === 'incompatible'}
                title={s.agent_compat ? `HarmonyAgent ${s.agent_compat} · ${s.permissions_json}` : s.compatibility_status}
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
        </>
      )}
    </div>
  )
}

/** 使用统计视图：调用汇总 + 技能排行 + 最近调用时间线 */
function SkillUsageView({
  skills,
  stats,
  events,
  loading,
  onRefresh,
  scope,
}: {
  skills: Skill[]
  stats: SkillUsageStat[]
  events: SkillUsageEvent[]
  loading: boolean
  onRefresh: () => void
  scope: 'global' | 'project'
}) {
  const { t } = useTranslation()

  // 汇总卡片
  const summary = useMemo(() => {
    const totalCalls = stats.reduce((s, u) => s + u.call_count, 0)
    const usedCount = stats.length
    const top = stats[0] ?? null
    return { totalCalls, usedCount, top }
  }, [stats])

  // 技能排行：全部已安装技能 + 调用统计合并（按次数降序；未调用过的排最后）
  const ranking = useMemo(() => {
    const byId = new Map(stats.map((s) => [s.skill_id, s]))
    const rows = skills.map((s) => ({
      skill: s,
      stat: byId.get(s.id) ?? null,
    }))
    // 已删除技能的调用记录也展示（用快照名），置于已安装之后
    const ghosts = stats
      .filter((s) => !skills.some((k) => k.id === s.skill_id))
      .map((s) => ({ skill: null as Skill | null, stat: s }))
    return [...rows, ...ghosts].sort((a, b) => {
      const ca = a.stat?.call_count ?? 0
      const cb = b.stat?.call_count ?? 0
      return cb - ca
    })
  }, [skills, stats])

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <p className="text-[11px] text-[var(--text-muted)]">
          {scope === 'project' ? t('skill.usageProjectHint') : t('skill.usageGlobalHint')}
        </p>
        <button
          onClick={onRefresh}
          className="shrink-0 px-3 py-1 text-xs border border-[var(--border)] rounded hover:bg-[var(--bg-card)] transition-colors"
        >
          {t('mcp.refresh')}
        </button>
      </div>

      {loading && <p className="text-xs text-[var(--text-muted)]">{t('common.loading')}</p>}

      {/* 汇总卡片 */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
        <div className="modern-card rounded-2xl p-4">
          <div className="text-[11px] text-[var(--text-muted)]">{t('skillStats.installedCount')}</div>
          <div className="text-2xl font-bold mt-1 tnum">{skills.length}</div>
        </div>
        <div className="modern-card rounded-2xl p-4">
          <div className="text-[11px] text-[var(--text-muted)]">{t('skillStats.totalCalls')}</div>
          <div className="text-2xl font-bold mt-1 tnum">{summary.totalCalls}</div>
        </div>
        <div className="modern-card rounded-2xl p-4">
          <div className="text-[11px] text-[var(--text-muted)]">{t('skillStats.usedCount')}</div>
          <div className="text-2xl font-bold mt-1 tnum">{summary.usedCount}</div>
        </div>
        <div className="modern-card rounded-2xl p-4">
          <div className="text-[11px] text-[var(--text-muted)]">{t('skillStats.mostUsed')}</div>
          {summary.top ? (
            <>
              <div className="text-lg font-semibold mt-1 truncate">{summary.top.skill_name}</div>
              <div className="text-[11px] text-[var(--accent)] mt-0.5 tnum">
                {t('skillStats.callsUnit', { n: summary.top.call_count })}
              </div>
            </>
          ) : (
            <div className="text-sm text-[var(--text-muted)] mt-1">{t('skillStats.noCalls')}</div>
          )}
        </div>
      </div>

      {/* 技能调用排行 */}
      <div className="modern-card rounded-2xl p-4">
        <div className="text-[13px] font-semibold mb-3">{t('skillStats.ranking')}</div>
        {ranking.length === 0 ? (
          <p className="text-xs text-[var(--text-muted)] py-6 text-center">{t('skill.empty')}</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-[12.5px]">
              <thead>
                <tr className="text-left text-[11px] text-[var(--text-muted)] border-b border-[var(--border)]">
                  <th className="py-2 pr-3 font-medium">{t('skillStats.colSkill')}</th>
                  <th className="py-2 pr-3 font-medium">{t('skillStats.colScope')}</th>
                  <th className="py-2 pr-3 font-medium">{t('skillStats.colStatus')}</th>
                  <th className="py-2 pr-3 font-medium text-right">{t('skillStats.colCalls')}</th>
                  <th className="py-2 font-medium">{t('skillStats.colLastCalled')}</th>
                </tr>
              </thead>
              <tbody>
                {ranking.map(({ skill, stat }) => (
                  <tr key={stat?.skill_id ?? skill!.id} className="border-b border-[var(--border)] last:border-b-0">
                    <td className="py-2 pr-3">
                      <span className="font-medium">{skill?.name ?? stat!.skill_name}</span>
                      {!skill && (
                        <span className="ml-1.5 px-1.5 py-0.5 rounded bg-[var(--bg-card)] text-[10px] text-[var(--text-muted)]">
                          {t('skillStats.removed')}
                        </span>
                      )}
                    </td>
                    <td className="py-2 pr-3 text-[var(--text-secondary)]">
                      {skill ? (skill.project_id ? t('common.scopeProject') : t('common.scopeGlobal')) : '—'}
                    </td>
                    <td className="py-2 pr-3">
                      {skill ? (
                        <span className={`px-2 py-0.5 rounded text-[11px] ${skill.enabled ? 'bg-[var(--success)]/15 text-[var(--success)]' : 'bg-[var(--bg-card)] text-[var(--text-secondary)]'}`}>
                          {skill.enabled ? t('skill.enabled') : t('skill.disabled')}
                        </span>
                      ) : (
                        '—'
                      )}
                    </td>
                    <td className="py-2 pr-3 text-right tnum font-semibold text-[var(--accent)]">
                      {stat?.call_count ?? 0}
                    </td>
                    <td className="py-2 text-[var(--text-secondary)] tnum">{fmtTime(stat?.last_called_at ?? null)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* 最近调用时间线 */}
      <div className="modern-card rounded-2xl p-4">
        <div className="text-[13px] font-semibold mb-3">{t('skillStats.recentCalls')}</div>
        {events.length === 0 ? (
          <p className="text-xs text-[var(--text-muted)] py-6 text-center">{t('skillStats.noEvents')}</p>
        ) : (
          <ul className="space-y-2">
            {events.map((e) => (
              <li key={e.id} className="flex items-center gap-3 text-[12.5px] py-1.5 border-b border-[var(--border)] last:border-b-0">
                <span className="shrink-0 w-24 text-[11px] text-[var(--text-muted)] tnum">{fmtTime(e.created_at)}</span>
                <span className="px-2 py-0.5 rounded bg-[var(--accent)]/10 text-[var(--accent)] text-[11px] font-medium shrink-0">
                  {e.skill_name}
                </span>
                <span className="truncate text-[var(--text-secondary)]">{e.conversation_title || '—'}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  )
}




