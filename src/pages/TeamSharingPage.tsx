// @ui-states: loading, empty, failed, retry
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useProjectStore } from '../stores/projectStore'
import { applyTeamShare, exportTeamShare, listTeamEvalSets, listTeamShareChanges, listTeamShareImports, previewTeamShare, revertTeamShare, runTeamEvalSet, type ShareChangeRecord, type ShareImportRecord, type SharePreview, type TeamEvalRun, type TeamEvalSetRecord } from '../api/teamSharing'

export default function TeamSharingPage() {
  const { t } = useTranslation()
  const project = useProjectStore((state) => state.currentProject)
  const [text, setText] = useState('')
  const [preview, setPreview] = useState<SharePreview | null>(null)
  const [imports, setImports] = useState<ShareImportRecord[]>([])
  const [sets, setSets] = useState<TeamEvalSetRecord[]>([])
  const [runs, setRuns] = useState<Record<string, TeamEvalRun>>({})
  const [changes, setChanges] = useState<Record<string, ShareChangeRecord[]>>({})
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [exportForm, setExportForm] = useState({ id: 'team-project', name: 'Team project context', version: '1.0.0', uri: 'urn:team:local', revision: 'draft' })

  const load = useCallback(async () => {
    if (!project) return
    setLoading(true)
    try {
      const [history, evalSets] = await Promise.all([listTeamShareImports(project.id), listTeamEvalSets(project.id)])
      setImports(history); setSets(evalSets)
    } catch (reason) { setError(String(reason)) } finally { setLoading(false) }
  }, [project])
  useEffect(() => { void load() }, [load])

  if (!project) return <div className="modern-card rounded-lg p-8 text-center text-sm text-[var(--text-secondary)]">{t('teamShare.selectProject')}</div>

  const parsed = () => JSON.parse(text) as unknown
  const inspect = async () => { setBusy(true); setError(null); try { setPreview(await previewTeamShare(project.id, parsed())) } catch (reason) { setError(String(reason)); setPreview(null) } finally { setBusy(false) } }
  const apply = async () => { if (!preview || !window.confirm(t('teamShare.applyConfirm'))) return; setBusy(true); setError(null); try { await applyTeamShare(project.id, parsed()); setPreview(null); await load() } catch (reason) { setError(String(reason)) } finally { setBusy(false) } }
  const revert = async (batchId: string) => { if (!window.confirm(t('teamShare.revertConfirm'))) return; setBusy(true); setError(null); try { await revertTeamShare(project.id, batchId); await load() } catch (reason) { setError(String(reason)) } finally { setBusy(false) } }
  const exportCurrent = async () => { setBusy(true); setError(null); try { const value = await exportTeamShare(project.id, exportForm.id, exportForm.name, exportForm.version, exportForm.uri, exportForm.revision); setText(JSON.stringify(value, null, 2)); setPreview(null) } catch (reason) { setError(String(reason)) } finally { setBusy(false) } }
  const runEval = async (setId: string) => { try { const result = await runTeamEvalSet(project.id, setId); setRuns((current) => ({ ...current, [setId]: result })) } catch (reason) { setError(String(reason)) } }
  const showChanges = async (batchId: string) => { try { const rows = await listTeamShareChanges(project.id, batchId); setChanges((current) => ({ ...current, [batchId]: rows })) } catch (reason) { setError(String(reason)) } }

  return <div className="space-y-5">
    <div><h2 className="text-xl font-semibold">{t('teamShare.title')}</h2><p className="text-xs text-[var(--text-secondary)] mt-1">{t('teamShare.desc')}</p></div>
    {error && <div className="rounded-lg border border-[var(--danger)]/40 bg-[var(--danger)]/10 p-3 text-xs text-[var(--danger)] break-all"><span>{error}</span><button disabled={loading} onClick={() => { setError(null); void load() }} className="ml-3 underline">{t('teamShare.retry')}</button></div>}
    {(loading || busy) && <div className="text-xs text-[var(--text-muted)]">{t(loading ? 'teamShare.loading' : 'teamShare.working')}</div>}
    <section className="modern-card rounded-xl p-4 space-y-3">
      <h3 className="font-medium">{t('teamShare.package')}</h3>
      <textarea value={text} onChange={(event) => { setText(event.target.value); setPreview(null) }} rows={12} className="w-full rounded-lg border border-[var(--border)] bg-transparent p-3 text-xs font-mono" placeholder={t('teamShare.packageHint')} />
      <div className="flex gap-2"><button disabled={busy || !text.trim()} onClick={() => void inspect()} className="px-3 h-8 rounded border border-[var(--accent)] text-[var(--accent)] text-xs">{t('teamShare.preview')}</button><button disabled={busy || !preview} onClick={() => void apply()} className="px-3 h-8 rounded bg-[var(--accent)] text-white text-xs">{t('teamShare.apply')}</button></div>
      {preview && <div className="rounded-lg bg-[var(--bg-card)] p-3 text-xs"><p>{t('teamShare.summary', { inserts: preview.inserts, updates: preview.updates, conflicts: preview.conflicts, unchanged: preview.unchanged })}</p><div className="mt-2 space-y-1 max-h-40 overflow-auto">{preview.items.map((item) => <p key={`${item.kind}:${item.key}`} className={item.action === 'conflict' ? 'text-[var(--warning)]' : 'text-[var(--text-secondary)]'}>{item.action} · {item.kind}:{item.key} · {item.reason}</p>)}</div></div>}
    </section>
    <section className="modern-card rounded-xl p-4 space-y-3"><h3 className="font-medium">{t('teamShare.export')}</h3><div className="grid grid-cols-2 gap-2">{Object.entries(exportForm).map(([key, value]) => <input key={key} value={value} onChange={(event) => setExportForm({ ...exportForm, [key]: event.target.value })} className="h-8 rounded border border-[var(--border)] bg-transparent px-2 text-xs" placeholder={key} />)}</div><button disabled={busy} onClick={() => void exportCurrent()} className="px-3 h-8 rounded border border-[var(--border)] text-xs">{t('teamShare.exportButton')}</button></section>
    <section className="space-y-2"><h3 className="font-medium">{t('teamShare.history')}</h3>{!loading && imports.length === 0 && <p className="text-xs text-[var(--text-muted)]">{t('teamShare.empty')}</p>}{imports.map((item) => <div key={item.batch_id} className="modern-card rounded-lg p-3"><div className="flex justify-between gap-3"><div className="text-xs"><p className="font-medium">{item.package_name} v{item.package_version} · {item.state}</p><p className="text-[var(--text-muted)] mt-1 break-all">{item.source_uri}@{item.source_revision} · {item.package_digest}</p></div><div className="flex gap-2"><button onClick={() => void showChanges(item.batch_id)} className="px-3 h-7 rounded border border-[var(--border)] text-xs">{t('teamShare.details')}</button>{item.state === 'applied' && <button disabled={busy} onClick={() => void revert(item.batch_id)} className="px-3 h-7 rounded border border-[var(--danger)] text-[var(--danger)] text-xs">{t('teamShare.revert')}</button>}</div></div>{changes[item.batch_id] && <div className="mt-2 border-t border-[var(--border)] pt-2 space-y-1">{changes[item.batch_id].map((change) => <p key={change.change_id} className="text-[10px] text-[var(--text-muted)]">{change.action} · {change.item_kind}:{change.stable_key} · {change.after_digest}</p>)}</div>}</div>)}</section>
    <section className="space-y-2"><h3 className="font-medium">{t('teamShare.evals')}</h3>{!loading && sets.length === 0 && <p className="text-xs text-[var(--text-muted)]">{t('teamShare.noEvals')}</p>}{sets.map((set) => <div key={set.id} className="modern-card rounded-lg p-3 flex justify-between"><div className="text-xs"><p className="font-medium">{set.name} v{set.version} · {set.case_count} cases</p>{runs[set.id] && <p className={runs[set.id].passed ? 'text-[var(--success)]' : 'text-[var(--danger)]'}>{runs[set.id].passed_cases}/{runs[set.id].total_cases} passed</p>}</div><button onClick={() => void runEval(set.id)} className="px-3 h-7 rounded border border-[var(--border)] text-xs">{t('teamShare.runEval')}</button></div>)}</section>
  </div>
}
