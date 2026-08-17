import { useState, useEffect, useCallback, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { listMcpServers, addMcpServer, updateMcpServer, testMcpServer, toggleMcpServer, removeMcpServer, cloneMcpServer, exportMcpConfig, importMcpConfig, fetchMcpFromUrl, listMcpUsageStats, type McpServer, type CreateMcpInput, type McpDraft, type McpUsageStat } from '../api/mcp'
import { mcpTemplates, matchMcpTemplate, templateEnvDefaults, type McpTemplate } from '../data/mcpTemplates'
import { useProjectStore } from '../stores/projectStore'

/**
 * 解析环境变量文本为对象（每行一个 KEY=value，兼容旧的逗号分隔）。
 * 按 "KEY=" 前缀扫描切分：值内可含逗号（如 Kafka 多 broker），
 * 空值（KEY=）与后出现的 KEY 覆盖前值。
 */
function parseEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {}
  const re = /([A-Za-z_][A-Za-z0-9_]*)\s*=\s*/g
  let last: { key: string; end: number } | null = null
  let m: RegExpExecArray | null
  while ((m = re.exec(text))) {
    if (last) env[last.key] = text.slice(last.end, m.index).trim().replace(/,\s*$/, '')
    last = { key: m[1], end: re.lastIndex }
  }
  if (last) env[last.key] = text.slice(last.end).trim().replace(/,\s*$/, '')
  return env
}

/** 将存储的 env JSON（{"KEY":"value"}）转为每行一个 "KEY=value" 的文本 */
function envToText(json: string): string {
  try {
    const obj = JSON.parse(json) as Record<string, string>
    return Object.entries(obj)
      .map(([k, v]) => `${k}=${v}`)
      .join('\n')
  } catch {
    return ''
  }
}

/** 格式化时间戳为可读文本 */
function formatTime(ts: number): string {
  const d = new Date(ts * 1000)
  return d.toLocaleString()
}

export default function McpPage() {
  const { t } = useTranslation()
  const currentProject = useProjectStore((s) => s.currentProject)
  // 全局项目（id='global'）也支持项目级专属配置（project_id='global' 行）
  const projectId = currentProject ? currentProject.id : null
  const [scope, setScope] = useState<'global' | 'project'>('global')
  // 未打开具体项目时，强制只显示全局
  const effectiveScope: 'global' | 'project' = projectId ? scope : 'global'
  const [servers, setServers] = useState<McpServer[]>([])
  const [showForm, setShowForm] = useState(false)
  const [form, setForm] = useState({ name: '', command: '', description: '', env: '' })
  const [addedKeys, setAddedKeys] = useState<Set<string>>(new Set())
  const [editingId, setEditingId] = useState<string | null>(null)
  const [testResult, setTestResult] = useState<Record<string, string>>({})
  const [fetchUrl, setFetchUrl] = useState('')
  const [fetchProxy, setFetchProxy] = useState(false)
  const [fetching, setFetching] = useState(false)
  const [drafts, setDrafts] = useState<McpDraft[] | null>(null)
  const [fetchError, setFetchError] = useState<string | null>(null)
  /** 视图切换：服务器列表 / 使用统计 */
  const [view, setView] = useState<'servers' | 'usage'>('servers')
  /** MCP 使用统计（按服务器聚合） */
  const [usageStats, setUsageStats] = useState<McpUsageStat[]>([])
  const [usageLoading, setUsageLoading] = useState(false)

  // useCallback 稳定引用：projectId 变化时 load 重建触发 effect，避免每次渲染重复加载
  const load = useCallback(async () => {
    try {
      // 加载全局 + 当前项目，前端按作用域 tab 过滤展示
      const list = await listMcpServers(projectId)
      setServers(list)
    } catch (e) {
      console.error(e)
    }
  }, [projectId])

  useEffect(() => { load() }, [load])

  // 使用统计：全局视图统计全部项目，项目视图仅当前项目
  const loadUsage = useCallback(async () => {
    setUsageLoading(true)
    try {
      const stats = await listMcpUsageStats(effectiveScope === 'project' ? projectId : null)
      setUsageStats(stats)
    } catch (e) {
      console.error(e)
      setUsageStats([])
    } finally {
      setUsageLoading(false)
    }
  }, [effectiveScope, projectId])

  useEffect(() => { void loadUsage() }, [loadUsage])

  const visibleServers = useMemo(
    () => servers.filter((s) => effectiveScope === 'global' ? !s.project_id : s.project_id === projectId),
    [servers, effectiveScope, projectId],
  )

  // 服务器名 → 使用统计（实例 #n 已在后端归并到基础名，同名实例共用同一统计）
  const usageMap = useMemo(() => {
    const map = new Map<string, McpUsageStat>()
    for (const u of usageStats) map.set(u.server_name.toLowerCase(), u)
    return map
  }, [usageStats])

  // 同名多实例编号（与后端 hint 规则一致：可见范围内按 project_id IS NOT NULL, id 排序编号），
  // 列表展示 name#n，Agent 调用时也使用该编号名（如 mcp__mysql#2__查询）
  const displayNames = useMemo(() => {
    const counts = new Map<string, number>()
    for (const s of servers) counts.set(s.name, (counts.get(s.name) ?? 0) + 1)
    const seen = new Map<string, number>()
    const names = new Map<string, string>()
    for (const s of servers) {
      if ((counts.get(s.name) ?? 1) > 1) {
        const idx = (seen.get(s.name) ?? 0) + 1
        seen.set(s.name, idx)
        names.set(s.id, `${s.name}#${idx}`)
      }
    }
    return names
  }, [servers])

  /** 一键添加内置模板（自动带出本机可用默认环境变量；已添加的显示标记） */
  const handleAddTemplate = async (tpl: McpTemplate) => {
    const defaults = templateEnvDefaults(tpl)
    await addMcpServer({
      name: tpl.name,
      command: tpl.command,
      env: defaults ? parseEnv(defaults) : undefined,
      description: tpl.description,
      homepage: tpl.homepage,
      project_id: effectiveScope === 'project' ? projectId : null,
    })
    setAddedKeys((k) => new Set(k).add(tpl.key))
    setTimeout(() => setAddedKeys((k) => { const n = new Set(k); n.delete(tpl.key); return n }), 2000)
    load()
  }

  /** 命令输入失焦：智能识别模板补全（含本机默认环境变量） */
  const handleCommandBlur = () => {
    const tpl = matchMcpTemplate(form.command)
    if (tpl) {
      setForm((f) => ({
        ...f,
        name: f.name || tpl.name,
        description: f.description || tpl.description,
        env: f.env || templateEnvDefaults(tpl),
      }))
    }
  }

  /** 添加或保存（编辑时按 id 走更新） */
  const handleSave = async () => {
    if (!form.name || !form.command) return
    const payload: CreateMcpInput = {
      name: form.name,
      command: form.command.split(' ').filter(Boolean),
      description: form.description || undefined,
      env: form.env.trim() ? parseEnv(form.env) : undefined,
      // 新增时按当前作用域归属；编辑不改变原有作用域
      project_id: editingId ? undefined : (effectiveScope === 'project' ? projectId : null),
    }
    if (editingId) {
      await updateMcpServer(editingId, payload)
    } else {
      await addMcpServer(payload)
    }
    setForm({ name: '', command: '', description: '', env: '' })
    setEditingId(null)
    setShowForm(false)
    load()
  }

  /** 打开编辑表单：预填当前连接配置 */
  const handleEdit = (s: McpServer) => {
    setEditingId(s.id)
    setForm({
      name: s.name,
      command: s.command,
      description: s.description ?? '',
      env: envToText(s.env),
    })
    setShowForm(true)
  }

  /** 测试连接：实际启动进程完成 MCP initialize 握手 */
  const handleTest = async (id: string) => {
    setTestResult((prev) => ({ ...prev, [id]: t('mcp.testing') }))
    try {
      const r = await testMcpServer(id)
      setTestResult((prev) => ({ ...prev, [id]: r }))
    } catch (e) {
      setTestResult((prev) => ({ ...prev, [id]: `${t('mcp.testFailed')}: ${e}` }))
    } finally {
      load() // 刷新健康状态徽章（测试结果已持久化）
    }
  }

  const handleToggle = async (id: string, enabled: boolean) => {
    await toggleMcpServer(id, !enabled)
    load()
  }

  /** 当前表单命令匹配到的模板环境变量说明（用于智能填写指导） */
  const tpl = matchMcpTemplate(form.command)
  const envDefs = tpl?.envDefs ?? []
  const envObj = parseEnv(form.env)

  /** 一键补齐缺失的环境变量（本机默认值填默认，其余留空待填；不覆盖已填值） */
  const fillEnvDefaults = () => {
    const cur = parseEnv(form.env)
    for (const d of envDefs) {
      if (cur[d.key] === undefined) cur[d.key] = d.defaultValue ?? ''
    }
    const text = Object.entries(cur)
      .map(([k, v]) => `${k}=${v}`)
      .join('\n')
    setForm((f) => ({ ...f, env: text }))
  }

  const handleRemove = async (id: string) => {
    if (!confirm(t('mcp.deleteConfirm'))) return
    await removeMcpServer(id)
    load()
  }

  const handleClone = async (s: McpServer) => {
    // 当前全局视图 → 复制到当前项目；当前项目视图 → 提升为全局
    const target = effectiveScope === 'global' ? projectId : null
    const targetLabel = target ? t('common.scopeProject') : t('common.scopeGlobal')
    if (!confirm(t('common.scopeCloneConfirm', { name: s.name, target: targetLabel }))) return
    try {
      await cloneMcpServer(s.id, target)
      load()
    } catch (e) {
      alert(String(e))
    }
  }

  const handleExport = async () => {
    try {
      const json = await exportMcpConfig(effectiveScope === 'project' ? projectId : null)
      const blob = new Blob([json], { type: 'application/json' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `mcp-config-${effectiveScope}-${Date.now()}.json`
      a.click()
      URL.revokeObjectURL(url)
    } catch (e) {
      alert(String(e))
    }
  }

  const importInputRef = useRef<HTMLInputElement | null>(null)
  const handleImportFile = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (file) {
      const reader = new FileReader()
      reader.onload = async () => {
        try {
          const n = await importMcpConfig(String(reader.result || ''), effectiveScope === 'project' ? projectId : null, false)
          alert(t('common.importDone', { n }))
          load()
        } catch (err) {
          alert(String(err))
        }
      }
      reader.readAsText(file)
    }
    e.target.value = ''
  }

  /** 从 URL 获取 MCP 配置 */
  const handleFetch = async () => {
    if (!fetchUrl.trim() || fetching) return
    setFetching(true)
    setFetchError(null)
    try {
      const list = await fetchMcpFromUrl(fetchUrl.trim(), fetchProxy)
      setDrafts(list)
    } catch (e) {
      setFetchError(String(e))
      setDrafts(null)
    } finally {
      setFetching(false)
    }
  }

  const draftToInput = (d: McpDraft): CreateMcpInput => ({
    name: d.name,
    command: d.command,
    env: d.env ?? undefined,
    description: d.description ?? undefined,
    project_id: effectiveScope === 'project' ? projectId : null,
  })

  const addDraft = async (d: McpDraft) => {
    await addMcpServer(draftToInput(d))
    load()
  }

  const addAllDrafts = async () => {
    if (!drafts) return
    for (const d of drafts) {
      await addMcpServer(draftToInput(d))
    }
    setDrafts(null)
    setFetchUrl('')
    load()
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-xl font-semibold">{t('mcp.title')}</h2>
          <p className="text-xs text-[var(--text-secondary)] mt-1">
            {view === 'usage' ? t('mcp.usageTip') : t('mcp.templateHint')}
          </p>
        </div>
        {view === 'servers' && (
          <button
            onClick={() => setShowForm(!showForm)}
            className="px-4 py-2 btn-primary rounded-lg text-sm transition-colors"
          >
            {showForm ? t('mcp.cancel') : t('mcp.add')}
          </button>
        )}
      </div>

      {/* 视图切换：服务器列表 / 使用统计 */}
      <div className="inline-flex modern-card rounded-lg p-0.5 mb-4 text-[12px]">
        <button
          onClick={() => setView('servers')}
          className={`px-3 h-7 rounded-md transition-colors ${view === 'servers' ? 'tab-active' : 'tab-inactive'}`}
        >
          {t('mcp.serversView')}
        </button>
        <button
          onClick={() => setView('usage')}
          className={`px-3 h-7 rounded-md transition-colors ${view === 'usage' ? 'tab-active' : 'tab-inactive'}`}
        >
          {t('mcp.usageView')}
        </button>
      </div>

      {view === 'usage' ? (
        <McpUsageView
          stats={usageStats}
          loading={usageLoading}
          onRefresh={() => void loadUsage()}
          projectId={projectId}
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

      <div className="flex items-center gap-2 mb-4">
        <button
          onClick={handleExport}
          className="h-7 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
        >
          {t('common.exportConfig')}
        </button>
        <button
          onClick={() => importInputRef.current?.click()}
          className="h-7 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
        >
          {t('common.importConfig')}
        </button>
        <input ref={importInputRef} type="file" accept="application/json,.json" className="hidden" onChange={handleImportFile} />
      </div>

      {showForm && (
        <div className="modern-card rounded-2xl p-4 mb-6 space-y-4 animate-fade-in-up">
          <div className="text-[13px] font-semibold text-[var(--text-primary)]">
            {editingId ? t('mcp.editTitle') : t('mcp.addTitle')}
          </div>
          {/* 内置模板选择 */}
          <div>
            <div className="text-[11px] font-medium text-[var(--text-muted)] mb-2">{t('mcp.templates')}</div>
            <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-2">
              {mcpTemplates.map((tpl) => {
                const installed = servers.some((s) => s.name.toLowerCase() === tpl.name.toLowerCase())
                const justAdded = addedKeys.has(tpl.key)
                const rec = tpl.recommended
                return (
                  <button
                    key={tpl.key}
                    onClick={() => handleAddTemplate(tpl)}
                    title={`${tpl.description}\n${tpl.popularity ? t('mcp.popularity', { data: tpl.popularity }) : ''}\n${tpl.envHint ? t('mcp.envSummary', { env: tpl.envHint }) : ''}\n${t('mcp.clickToAdd')}`}
                    className={`px-2.5 py-1.5 rounded-lg border text-left transition-all ${
                      installed || justAdded
                        ? 'border-[var(--success)]/50 bg-[var(--success)]/10'
                        : 'border-[var(--border)] bg-[var(--bg-card)] hover:border-[var(--accent)]/40 hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    <span
                      className={`block text-[12px] truncate ${
                        installed || justAdded ? 'text-[var(--success)]' : 'text-[var(--text-primary)]'
                      }`}
                    >
                      {justAdded ? t('mcp.added') : tpl.name}
                      {rec === 'hot' && !justAdded && (
                        <span className="ml-1 px-1 py-px rounded bg-[#f59e0b]/15 text-[#f59e0b] text-[9px] font-bold align-middle">🔥 {t('mcp.hot')}</span>
                      )}
                      {rec === 'popular' && !justAdded && (
                        <span className="ml-1 px-1 py-px rounded bg-[var(--accent)]/10 text-[var(--accent)] text-[9px] font-bold align-middle">⭐ {t('mcp.popular')}</span>
                      )}
                      {installed && !justAdded && ' ✓'}
                    </span>
                  </button>
                )
              })}
            </div>
            <p className="text-[10px] text-[var(--text-muted)]">{t('mcp.oneClickHint')}</p>
            <p className="text-[10px] text-[var(--text-muted)] mt-0.5">{t('mcp.recommendedTip')}</p>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <label className="text-[11px] text-[var(--text-muted)]">{t('mcp.name')}</label>
              <input
                placeholder="playwright"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                className="w-full h-9 px-3 modern-card rounded-lg text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-[11px] text-[var(--text-muted)]">{t('mcp.command')}</label>
              <input
                placeholder="npx @playwright/mcp@latest"
                value={form.command}
                onChange={(e) => setForm({ ...form, command: e.target.value })}
                onBlur={handleCommandBlur}
                className="w-full h-9 px-3 modern-card rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-[11px] text-[var(--text-muted)]">{t('mcp.description')}</label>
              <input
                placeholder={t('mcp.description')}
                value={form.description}
                onChange={(e) => setForm({ ...form, description: e.target.value })}
                className="w-full h-9 px-3 modern-card rounded-lg text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-[11px] text-[var(--text-muted)]">{t('mcp.env')}</label>
              <textarea
                placeholder={t('mcp.envPlaceholder')}
                value={form.env}
                onChange={(e) => setForm({ ...form, env: e.target.value })}
                rows={Math.max(3, Math.min(6, envDefs.length + 1))}
                className="w-full px-3 py-2 modern-card rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)] resize-y"
              />
            </div>
          </div>

          {/* 环境变量智能说明：识别到模板时逐项展示用途/示例/是否已配置 */}
          {envDefs.length > 0 && (
            <div className="rounded-lg modern-card p-3 space-y-1.5">
              <div className="flex items-center justify-between gap-2">
                <span className="text-[11px] font-medium text-[var(--text-muted)]">{t('mcp.envGuideTitle')}</span>
                <button
                  onClick={fillEnvDefaults}
                  className="text-[11px] text-[var(--accent)] hover:underline shrink-0"
                >
                  {t('mcp.fillDefaults')}
                </button>
              </div>
              {envDefs.map((d) => {
                const configured = envObj[d.key] !== undefined && envObj[d.key] !== ''
                return (
                  <div key={d.key} className="flex items-baseline gap-2 text-[11px] leading-snug">
                    <span className={`font-mono shrink-0 ${configured ? 'text-[var(--success)]' : 'text-[var(--text-secondary)]'}`}>
                      {d.key}
                    </span>
                    <span className="flex-1 min-w-0 text-[var(--text-muted)]">{d.hint}</span>
                    {configured ? (
                      <span className="text-[var(--success)] shrink-0">✓</span>
                    ) : (
                      <span className="text-[var(--warning)] shrink-0 font-mono">{d.placeholder}</span>
                    )}
                  </div>
                )
              })}
            </div>
          )}

          <button onClick={handleSave} className="px-4 py-2 bg-[var(--success)] text-white rounded text-sm hover:opacity-90 transition-opacity">
            {t('mcp.save')}
          </button>
        </div>
      )}

      {/* 从 URL 导入（支持走系统代理） */}
      <div className="modern-card rounded-2xl p-4 mb-6 space-y-3 animate-fade-in-up">
        <div className="text-[11px] font-medium text-[var(--text-muted)]">{t('mcp.fromUrl')}</div>
        <div className="flex gap-2">
          <input
            placeholder={t('mcp.urlPlaceholder')}
            value={fetchUrl}
            onChange={(e) => setFetchUrl(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleFetch()}
            className="flex-1 h-9 px-3 modern-card rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
          />
          <button
            onClick={handleFetch}
            disabled={!fetchUrl.trim() || fetching}
            className="px-4 h-9 rounded-[10px] btn-primary text-[13px] font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
          >
            {fetching ? t('mcp.fetching') : t('mcp.fetch')}
          </button>
        </div>
        <label className="flex items-center gap-2 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={fetchProxy}
            onChange={(e) => setFetchProxy(e.target.checked)}
            className="w-3.5 h-3.5 accent-[var(--accent)]"
          />
          <span className="text-[12px] text-[var(--text-secondary)]">{t('mcp.useProxy')}</span>
        </label>
        {fetchError && <p className="text-xs text-[var(--danger)] break-all">{fetchError}</p>}
        {drafts && (
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-[12px] text-[var(--text-secondary)]">
                {t('mcp.fetchedCount', { count: drafts.length })}
              </span>
              <button
                onClick={addAllDrafts}
                className="px-3 h-7 rounded-lg bg-[var(--success)] text-white text-[12px] font-medium hover:opacity-90 transition-opacity"
              >
                {t('mcp.addAll')}
              </button>
            </div>
            {drafts.map((d) => (
              <div key={d.name} className="flex items-center justify-between gap-3 modern-card rounded-lg px-3 py-2">
                <div className="min-w-0">
                  <div className="text-[13px] font-medium truncate">{d.name}</div>
                  <div className="text-[11px] text-[var(--text-secondary)] font-mono truncate">{d.command.join(' ')}</div>
                  {d.description && <div className="text-[11px] text-[var(--text-muted)] truncate">{d.description}</div>}
                </div>
                <button
                  onClick={() => addDraft(d)}
                  className="px-3 py-1 text-xs border border-[var(--accent)] text-[var(--accent)] rounded hover:bg-[var(--accent-soft)] transition-colors shrink-0"
                >
                  {t('mcp.add')}
                </button>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="space-y-3">
        {visibleServers.length === 0 && (
          <p className="text-[var(--text-secondary)] text-sm">{t('mcp.empty')}</p>
        )}
        <p className="text-[10px] text-[var(--text-muted)]">{t('mcp.multiInstanceHint')}</p>
        {visibleServers.map((s) => (
          <div key={s.id} className="modern-card rounded-lg p-4 flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <span className="font-medium">{displayNames.get(s.id) ?? s.name}</span>
                <span className={`text-xs px-2 py-0.5 rounded ${s.enabled ? 'bg-[var(--success)] text-white' : 'bg-[var(--bg-card)] text-[var(--text-secondary)]'}`}>
                  {s.enabled ? t('mcp.enable') : t('mcp.disable')}
                </span>
                {s.enabled && (
                  <span
                    title={t('mcp.healthTitle', {
                      error: s.last_test_ok ? '' : `${s.last_test_error ?? ''}\n`,
                      time: s.last_test_at ? formatTime(s.last_test_at) : '-',
                    })}
                    className={`text-xs px-2 py-0.5 rounded border ${
                      s.last_test_at === null
                        ? 'bg-[var(--bg-card)] text-[var(--text-secondary)] border-[var(--border)]'
                        : s.last_test_ok
                          ? 'bg-[var(--success)]/15 text-[var(--success)] border-[var(--success)]/40'
                          : 'bg-[var(--danger)]/15 text-[var(--danger)] border-[var(--danger)]/40'
                    }`}
                  >
                    {s.last_test_at === null ? t('mcp.notTested') : s.last_test_ok ? t('mcp.healthOk') : t('mcp.healthBad')}
                  </span>
                )}
              </div>
              {s.description && <p className="text-xs text-[var(--text-secondary)] mt-1">{s.description}</p>}
              <p className="text-xs text-[var(--text-secondary)] mt-1 font-mono break-all">{s.command}</p>
              {envToText(s.env) && (
                <p className="text-[10px] text-[var(--text-muted)] mt-1 font-mono break-all">
                  {t('mcp.envSummary', { env: envToText(s.env).replace(/\n/g, ', ') })}
                </p>
              )}
              <p className="text-[10px] text-[var(--text-muted)] mt-0.5">{t('mcp.createdAt', { time: formatTime(s.created_at) })}</p>
              {usageMap.get(s.name.toLowerCase()) && (
                <UsageSummaryLine stat={usageMap.get(s.name.toLowerCase())!} />
              )}
              {testResult[s.id] && (
                <p className={`text-xs mt-1 font-mono break-all ${testResult[s.id].includes('成功') || testResult[s.id].startsWith('Connection') ? 'text-[var(--success)]' : 'text-[var(--danger)]'}`}>
                  {testResult[s.id]}
                </p>
              )}
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <button
                onClick={() => handleTest(s.id)}
                className="px-3 py-1 text-xs border border-[var(--border)] rounded hover:bg-[var(--bg-card)] transition-colors"
              >
                {t('mcp.test')}
              </button>
              <button
                onClick={() => (editingId === s.id ? setEditingId(null) : handleEdit(s))}
                className="px-3 py-1 text-xs border border-[var(--accent)] text-[var(--accent)] rounded hover:bg-[var(--accent-soft)] transition-colors"
              >
                {t('mcp.edit')}
              </button>
              <button
                onClick={() => handleToggle(s.id, s.enabled)}
                className="px-3 py-1 text-xs border border-[var(--border)] rounded hover:bg-[var(--bg-card)] transition-colors"
              >
                {s.enabled ? t('mcp.disable') : t('mcp.enable')}
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
                onClick={() => handleRemove(s.id)}
                className="px-3 py-1 text-xs border border-[var(--danger)] text-[var(--danger)] rounded hover:bg-[var(--danger)] hover:text-white transition-colors"
              >
                {t('mcp.delete')}
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

/** 服务器卡片上的调用情况摘要行 */
function UsageSummaryLine({ stat }: { stat: McpUsageStat }) {
  const { t } = useTranslation()
  const rate = stat.call_count > 0 ? Math.round((stat.success_count / stat.call_count) * 100) : 0
  return (
    <div
      className="mt-1.5 flex items-center gap-2 text-[10px] text-[var(--text-muted)]"
      title={t('mcp.usageTip')}
    >
      <span className="tabular-nums">{t('mcp.calls', { n: stat.call_count })}</span>
      <span className="tabular-nums text-[var(--success)]">{t('mcp.success', { n: stat.success_count })}</span>
      {stat.fail_count > 0 && (
        <span className="tabular-nums text-[var(--danger)]">{t('mcp.failed', { n: stat.fail_count })}</span>
      )}
      <span className="tabular-nums">{t('mcp.rate', { rate })}</span>
      {stat.avg_duration_ms != null && (
        <span className="tabular-nums">{t('mcp.avgMs', { ms: stat.avg_duration_ms })}</span>
      )}
      {stat.last_called_at != null && (
        <span className="tabular-nums">{t('mcp.lastAt', { time: formatTime(stat.last_called_at) })}</span>
      )}
    </div>
  )
}

/** 使用统计视图：按服务器聚合的调用汇总 + 工具明细 */
function McpUsageView({
  stats,
  loading,
  onRefresh,
  projectId,
}: {
  stats: McpUsageStat[]
  loading: boolean
  onRefresh: () => void
  projectId: string | null
}) {
  const { t } = useTranslation()
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const totalCalls = stats.reduce((s, x) => s + x.call_count, 0)
  const totalFail = stats.reduce((s, x) => s + x.fail_count, 0)

  const toggle = (name: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }

  if (loading) {
    return <p className="text-sm text-[var(--text-secondary)]">{t('mcp.loadingStats')}</p>
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <p className="text-[11px] text-[var(--text-muted)]">
          {projectId ? t('mcp.usageProjectHint') : t('mcp.usageGlobalHint')}
        </p>
        <button
          onClick={onRefresh}
          className="shrink-0 px-3 py-1 text-xs border border-[var(--border)] rounded hover:bg-[var(--bg-card)] transition-colors"
        >
          {t('mcp.refresh')}
        </button>
      </div>

      {stats.length === 0 ? (
        <div className="rounded-xl border border-dashed border-[var(--border)] p-8 text-center">
          <p className="text-[13px] text-[var(--text-muted)]">{t('mcp.usageEmpty')}</p>
        </div>
      ) : (
        <>
          {/* 汇总 */}
          <div className="grid grid-cols-3 gap-2">
            <div className="rounded-xl modern-card p-2.5 text-center">
              <div className="text-lg font-semibold tabular-nums">{totalCalls}</div>
              <div className="text-[10px] text-[var(--text-muted)]">{t('mcp.usageTotalCalls')}</div>
            </div>
            <div className="rounded-xl modern-card p-2.5 text-center">
              <div className="text-lg font-semibold tabular-nums text-[var(--success)]">{totalCalls - totalFail}</div>
              <div className="text-[10px] text-[var(--text-muted)]">{t('mcp.usageSuccess')}</div>
            </div>
            <div className="rounded-xl modern-card p-2.5 text-center">
              <div className="text-lg font-semibold tabular-nums text-[var(--danger)]">{totalFail}</div>
              <div className="text-[10px] text-[var(--text-muted)]">{t('mcp.usageFailed')}</div>
            </div>
          </div>

          {/* 按服务器明细 */}
          {stats.map((s) => {
            const isCollapsed = collapsed.has(s.server_name)
            const rate = s.call_count > 0 ? Math.round((s.success_count / s.call_count) * 100) : 0
            return (
              <div key={s.server_name} className="rounded-xl modern-card overflow-hidden">
                <button
                  onClick={() => toggle(s.server_name)}
                  className="w-full flex items-center justify-between gap-2 px-3 py-2.5 text-left hover:bg-[var(--bg-hover)] transition-colors"
                >
                  <span className="flex items-center gap-2 min-w-0">
                    <span className={`text-[11px] transition-transform ${isCollapsed ? '' : 'rotate-90'}`}>▶</span>
                    <span className="text-[13px] font-medium truncate">{s.server_name}</span>
                    <span className="shrink-0 text-[11px] tabular-nums text-[var(--text-muted)]">
                      {s.call_count}×
                    </span>
                  </span>
                  <span className="flex items-center gap-3 shrink-0 text-[10px] tabular-nums text-[var(--text-muted)]">
                    <span className="text-[var(--success)]">{s.success_count} ✓</span>
                    {s.fail_count > 0 && <span className="text-[var(--danger)]">{s.fail_count} ✗</span>}
                    <span>{t('mcp.rate', { rate })}</span>
                    {s.avg_duration_ms != null && <span>{t('mcp.avgMs', { ms: s.avg_duration_ms })}</span>}
                  </span>
                </button>
                {!isCollapsed && s.tools.length > 0 && (
                  <div className="px-3 pb-3 space-y-1.5">
                    {s.tools.map((tool) => {
                      const tRate = tool.call_count > 0 ? Math.round((tool.success_count / tool.call_count) * 100) : 0
                      return (
                        <div key={tool.tool_name} className="rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] p-2">
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-[11px] font-mono truncate">{tool.tool_name}</span>
                            <span className="shrink-0 text-[10px] tabular-nums">{tool.call_count}×</span>
                          </div>
                          <div className="mt-1.5 h-1 rounded-full bg-[var(--bg-hover)] overflow-hidden">
                            <div
                              className={`h-full rounded-full ${tRate >= 80 ? 'bg-[var(--success)]' : tRate >= 50 ? 'bg-[var(--warning)]' : 'bg-[var(--danger)]'}`}
                              style={{ width: `${tRate}%` }}
                            />
                          </div>
                          <div className="mt-1 flex items-center justify-between text-[9px] text-[var(--text-muted)]">
                            <span>
                              {t('mcp.rate', { rate: tRate })} · {t('mcp.avgMs', { ms: tool.avg_duration_ms ?? '—' })}
                            </span>
                            {tool.last_called_at != null && (
                              <span>{formatTime(tool.last_called_at)}</span>
                            )}
                          </div>
                        </div>
                      )
                    })}
                  </div>
                )}
              </div>
            )
          })}
        </>
      )}
    </div>
  )
}





