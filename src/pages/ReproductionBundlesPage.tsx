import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { generateReproductionBundle, listReproductionBundles, previewReproductionBundle, validateReproductionBundle, type ReproductionBundleRecord, type ReproductionPreview, type ReproductionRequest } from '../api/reproductionBundle'
import { useProjectStore } from '../stores/projectStore'

const emptyForm = {
  title: '', description: '', steps: '', expected: '', actual: '', runId: '', attachments: '',
  includeMessages: true, includeToolRuns: true, includeRunEvents: true,
}

export default function ReproductionBundlesPage() {
  const { t } = useTranslation()
  const project = useProjectStore((state) => state.currentProject)
  const conversation = useProjectStore((state) => state.currentConversation)
  const [form, setForm] = useState(emptyForm)
  const [preview, setPreview] = useState<ReproductionPreview | null>(null)
  const [records, setRecords] = useState<ReproductionBundleRecord[]>([])
  const [validation, setValidation] = useState<Record<string, string>>({})
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const request = useMemo<ReproductionRequest>(() => ({
    title: form.title,
    description: form.description,
    steps: form.steps.split('\n').map((line) => line.trim()).filter(Boolean),
    expected: form.expected,
    actual: form.actual,
    conversation_id: form.runId.trim() ? null : conversation && conversation.project_id === project?.id ? conversation.id : null,
    run_id: form.runId.trim() || null,
    include_messages: form.includeMessages,
    include_tool_runs: form.includeToolRuns,
    include_run_events: form.includeRunEvents,
    attachments: form.attachments.split('\n').map((line) => line.trim()).filter(Boolean),
  }), [conversation, form, project])

  const load = useCallback(async () => {
    if (!project) return
    setLoading(true)
    try { setRecords(await listReproductionBundles(project.id)) }
    catch (reason) { setError(String(reason)) }
    finally { setLoading(false) }
  }, [project])
  useEffect(() => { void load() }, [load])

  if (!project) return <div className="modern-card rounded-lg p-8 text-center text-sm text-[var(--text-secondary)]">{t('repro.selectProject')}</div>
  const update = (key: keyof typeof form, value: string | boolean) => { setForm((current) => ({ ...current, [key]: value })); setPreview(null) }
  const inspect = async () => { setBusy(true); setError(null); try { setPreview(await previewReproductionBundle(project.id, request)) } catch (reason) { setError(String(reason)); setPreview(null) } finally { setBusy(false) } }
  const generate = async () => { if (!preview || !window.confirm(t('repro.confirm', { count: preview.entries.length, size: preview.total_bytes }))) return; setBusy(true); setError(null); try { await generateReproductionBundle(project.id, request, preview.preview_digest); setPreview(null); await load() } catch (reason) { setError(String(reason)) } finally { setBusy(false) } }
  const validate = async (record: ReproductionBundleRecord) => { setError(null); try { const result = await validateReproductionBundle(project.id, record.bundle_id); setValidation((current) => ({ ...current, [record.bundle_id]: result.valid ? t('repro.valid') : t('repro.invalid') })) } catch (reason) { setValidation((current) => ({ ...current, [record.bundle_id]: t('repro.invalid') })); setError(String(reason)) } }

  return <div className="space-y-5">
    <div><h2 className="text-xl font-semibold">{t('repro.title')}</h2><p className="mt-1 text-xs text-[var(--text-secondary)]">{t('repro.desc')}</p></div>
    {error && <div className="rounded-lg border border-[var(--danger)]/40 bg-[var(--danger)]/10 p-3 text-xs text-[var(--danger)]"><span className="break-all">{error}</span><button disabled={loading} onClick={() => { setError(null); void load() }} className="ml-3 underline">{t('repro.retry')}</button></div>}
    {(loading || busy) && <p className="text-xs text-[var(--text-muted)]">{t(loading ? 'repro.loading' : 'repro.working')}</p>}
    <section className="modern-card rounded-xl p-4 space-y-3">
      <div className="grid grid-cols-2 gap-3"><label className="text-xs">{t('repro.issueTitle')}<input value={form.title} onChange={(event) => update('title', event.target.value)} className="mt-1 h-9 w-full rounded border border-[var(--border)] bg-transparent px-2" /></label><label className="text-xs">{t('repro.runId')}<input value={form.runId} onChange={(event) => update('runId', event.target.value)} placeholder={t('repro.optional')} className="mt-1 h-9 w-full rounded border border-[var(--border)] bg-transparent px-2" /></label></div>
      <label className="block text-xs">{t('repro.description')}<textarea rows={3} value={form.description} onChange={(event) => update('description', event.target.value)} className="mt-1 w-full rounded border border-[var(--border)] bg-transparent p-2" /></label>
      <div className="grid grid-cols-3 gap-3"><label className="text-xs">{t('repro.steps')}<textarea rows={5} value={form.steps} onChange={(event) => update('steps', event.target.value)} placeholder={t('repro.onePerLine')} className="mt-1 w-full rounded border border-[var(--border)] bg-transparent p-2" /></label><label className="text-xs">{t('repro.expected')}<textarea rows={5} value={form.expected} onChange={(event) => update('expected', event.target.value)} className="mt-1 w-full rounded border border-[var(--border)] bg-transparent p-2" /></label><label className="text-xs">{t('repro.actual')}<textarea rows={5} value={form.actual} onChange={(event) => update('actual', event.target.value)} className="mt-1 w-full rounded border border-[var(--border)] bg-transparent p-2" /></label></div>
      <label className="block text-xs">{t('repro.attachments')}<textarea rows={3} value={form.attachments} onChange={(event) => update('attachments', event.target.value)} placeholder={t('repro.attachmentHint')} className="mt-1 w-full rounded border border-[var(--border)] bg-transparent p-2 font-mono" /></label>
      <div className="flex flex-wrap gap-4 text-xs">{([['includeMessages', 'repro.messages'], ['includeToolRuns', 'repro.toolRuns'], ['includeRunEvents', 'repro.runEvents']] as const).map(([key, label]) => <label key={key} className="flex items-center gap-2"><input type="checkbox" checked={form[key]} onChange={(event) => update(key, event.target.checked)} />{t(label)}</label>)}</div>
      <p className="text-[11px] text-[var(--text-muted)]">{conversation ? t('repro.currentConversation', { title: conversation.title }) : t('repro.noConversation')}</p>
      <div className="flex gap-2"><button disabled={busy || !form.title.trim()} onClick={() => void inspect()} className="h-8 rounded border border-[var(--accent)] px-3 text-xs text-[var(--accent)]">{t('repro.preview')}</button><button disabled={busy || !preview} onClick={() => void generate()} className="h-8 rounded bg-[var(--accent)] px-3 text-xs text-white">{t('repro.generate')}</button></div>
      {preview && <div className="rounded-lg bg-[var(--bg-card)] p-3 text-xs space-y-2"><p>{t('repro.previewSummary', { count: preview.entries.length, size: preview.total_bytes, redacted: preview.redacted_entry_count })}</p><p className="break-all text-[10px] text-[var(--text-muted)]">{preview.preview_digest}</p>{preview.entries.map((entry) => <p key={entry.path} className="text-[11px]">{entry.redacted ? '✓' : '—'} {entry.path} · {entry.bytes} B · {entry.kind}</p>)}{preview.omitted_attachments.map((item) => <p key={item} className="text-[var(--warning)]">{t('repro.omitted')}: {item}</p>)}{preview.warnings.map((item) => <p key={item} className="text-[var(--warning)]">{item}</p>)}</div>}
    </section>
    <section className="space-y-2"><h3 className="font-medium">{t('repro.history')}</h3>{!loading && records.length === 0 && <p className="text-xs text-[var(--text-muted)]">{t('repro.empty')}</p>}{records.map((record) => <div key={record.bundle_id} className="modern-card rounded-lg p-3 text-xs"><div className="flex justify-between gap-3"><div><p className="font-medium">{record.title}</p><p className="mt-1 break-all text-[var(--text-muted)]">{record.archive_rel_path} · {record.archive_bytes} B · {record.entry_count} entries</p><p className="break-all text-[10px] text-[var(--text-muted)]">{record.archive_sha256}</p>{validation[record.bundle_id] && <p className="text-[var(--success)]">{validation[record.bundle_id]}</p>}</div><button onClick={() => void validate(record)} className="h-7 rounded border border-[var(--border)] px-3">{t('repro.validate')}</button></div></div>)}</section>
  </div>
}
