import { useState, useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import {
  listProviders,
  createProvider,
  updateProvider,
  deleteProvider,
  switchProvider,
  testProvider,
  listProviderModels,
  updateModel,
  addModel,
  removeModel,
  type Provider,
  type CreateProviderInput,
  type ProviderModel,
  type EndpointDef,
} from '../api/provider'
import {
  providerTemplates,
  templateCategories,
  matchTemplateByUrl,
  type ProviderTemplate,
} from '../data/providerTemplates'
import Icon from '../icons/Icon'

/** 模态类型：与后端 input_modalities/output_modalities JSON 数组对应 */
const MODALITY_OPTIONS = ['text', 'image', 'audio', 'video'] as const
type Modality = (typeof MODALITY_OPTIONS)[number]

/** 模型类型预设：一键设置输入/输出模态（单选） */
const MODALITY_PRESETS: { key: string; input: Modality[]; output: Modality[] }[] = [
  { key: 'text', input: ['text'], output: ['text'] },
  { key: 'image', input: ['image'], output: ['image'] },
  { key: 'audio', input: ['audio'], output: ['audio'] },
  { key: 'video', input: ['video'], output: ['video'] },
  { key: 'omni', input: ['text', 'image', 'audio', 'video'], output: ['text', 'image', 'audio', 'video'] },
]

/** 解析模态 JSON 字符串为数组（非法值回退 text） */
function parseModalities(s: string | null | undefined): Modality[] {
  try {
    const arr = JSON.parse(s ?? '["text"]')
    return Array.isArray(arr)
      ? arr.filter((x): x is Modality => (MODALITY_OPTIONS as readonly string[]).includes(x))
      : ['text']
  } catch {
    return ['text']
  }
}

/** 类型徽标短文本（文/图/音/视，多模态叠加） */
function modalityShort(s: string | null | undefined, t: (k: string) => string): string {
  const map: Record<string, string> = {
    text: t('provider.modShort.text'),
    image: t('provider.modShort.image'),
    audio: t('provider.modShort.audio'),
    video: t('provider.modShort.video'),
  }
  return parseModalities(s)
    .map((m) => map[m] ?? '')
    .join('') || map.text
}

/** 模型类型选择器：预设单选 + 输入/输出模态多选 */
function ModalityPicker({
  preset,
  inMods,
  outMods,
  onPreset,
  onToggle,
}: {
  preset: string
  inMods: Modality[]
  outMods: Modality[]
  onPreset: (key: string) => void
  onToggle: (side: 'in' | 'out', mo: Modality) => void
}) {
  const { t } = useTranslation()
  const row = (side: 'in' | 'out', mods: Modality[]) => (
    <>
      <span className="text-[10px] text-[var(--text-muted)]">
        {side === 'in' ? t('provider.modInput') : t('provider.modOutput')}
      </span>
      {MODALITY_OPTIONS.map((mo) => (
        <label key={mo} className="flex items-center gap-1 text-[10px] text-[var(--text-secondary)] cursor-pointer select-none">
          <input
            type="checkbox"
            checked={mods.includes(mo)}
            onChange={() => onToggle(side, mo)}
            className="w-3 h-3 accent-[var(--accent)]"
          />
          {t(`provider.mod.${mo}`)}
        </label>
      ))}
    </>
  )
  return (
    <div>
      <div className="flex gap-1.5 flex-wrap">
        {MODALITY_PRESETS.map((p) => (
          <button
            key={p.key}
            onClick={() => onPreset(p.key)}
            className={`px-2 py-0.5 text-[10px] rounded-md border transition-colors ${
              preset === p.key
                ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]'
                : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text-primary)]'
            }`}
          >
            {t(`provider.modPreset.${p.key}`)}
          </button>
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 mt-1.5">
        {row('in', inMods)}
        <span className="text-[var(--border)]">|</span>
        {row('out', outMods)}
      </div>
    </div>
  )
}
/** 多协议端点编辑器：协议 + URL 行列表，可增删（如 DeepSeek 的 OpenAI/Anthropic 两套端点） */
function EndpointEditor({
  endpoints,
  onChange,
}: {
  endpoints: EndpointDef[]
  onChange: (next: EndpointDef[]) => void
}) {
  const { t } = useTranslation()
  const [proto, setProto] = useState('openai')
  const [url, setUrl] = useState('')
  const add = () => {
    const u = url.trim()
    if (!u) return
    onChange([...endpoints, { protocol: proto, base_url: u }])
    setUrl('')
  }
  return (
    <div className="space-y-1.5">
      <label className="text-[11px] text-[var(--text-muted)]">{t('provider.endpoints')}</label>
      <div className="space-y-1.5">
        {endpoints.map((ep, i) => (
          <div key={i} className="flex items-center gap-2">
            <span className="w-20 h-8 px-2 rounded-lg bg-[var(--accent-soft)] text-[var(--accent)] text-[11px] font-mono flex items-center justify-center shrink-0">
              {ep.protocol}
            </span>
            <span className="flex-1 min-w-0 h-8 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[12px] font-mono text-[var(--text-primary)] flex items-center truncate">
              {ep.base_url}
            </span>
            <button
              onClick={() => onChange(endpoints.filter((_, j) => j !== i))}
              className="h-8 px-2 text-[var(--text-muted)] hover:text-[var(--danger)] transition-colors shrink-0"
              title={t('provider.endpointDelete')}
            >
              <Icon name="close" size={12} />
            </button>
          </div>
        ))}
      </div>
      <div className="flex items-center gap-2">
        <select
          value={proto}
          onChange={(e) => setProto(e.target.value)}
          className="w-24 h-8 px-2 rounded-lg bg-[var(--bg-card)] border border-[var(--border)] text-[11px] font-mono text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
        >
          <option value="openai">{t('provider.protoOpenai')}</option>
          <option value="anthropic">{t('provider.protoAnthropic')}</option>
          <option value="gemini">{t('provider.protoGemini')}</option>
        </select>
        <input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              add()
            }
          }}
          placeholder="https://api.example.com/anthropic"
          className="flex-1 min-w-0 h-8 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[12px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
        />
        <button
          onClick={add}
          className="h-8 px-3 text-[11px] border border-[var(--accent)] text-[var(--accent)] rounded-lg hover:bg-[var(--accent-soft)] transition-colors shrink-0"
        >
          {t('provider.endpointAdd')}
        </button>
      </div>
      <p className="text-[10px] text-[var(--text-muted)]">{t('provider.endpointsHint')}</p>
    </div>
  )
}


export default function ProvidersPage() {
  const { t } = useTranslation()
  const [providers, setProviders] = useState<Provider[]>([])
  const [modelsMap, setModelsMap] = useState<Record<string, ProviderModel[]>>({})
  const [showForm, setShowForm] = useState(false)
  const [testResult, setTestResult] = useState<Record<string, string>>({})
  const [error, setError] = useState<string | null>(null)
  const [form, setForm] = useState<CreateProviderInput>({
    name: '',
    provider_type: 'openai-compatible',
    protocol: 'openai',
    base_url: '',
    api_key: '',
    endpoints: [],
  })
  const [models, setModels] = useState<string[]>([])
  const [modelInput, setModelInput] = useState('')
  const [keyHint, setKeyHint] = useState('')
  const [selectedTpl, setSelectedTpl] = useState<string | null>(null)
  const formRef = useRef<HTMLDivElement>(null)
  // 编辑状态
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editForm, setEditForm] = useState({ name: '', protocol: 'openai', base_url: '', api_key: '', endpoints: [] as EndpointDef[] })
  const [editModels, setEditModels] = useState<ProviderModel[]>([])
  const [editModelInput, setEditModelInput] = useState('')
  // 新添加模型的类型（预设 + 输入/输出模态多选）
  const [editModPreset, setEditModPreset] = useState('text')
  const [editModIn, setEditModIn] = useState<Modality[]>(['text'])
  const [editModOut, setEditModOut] = useState<Modality[]>(['text'])
  const [editSaving, setEditSaving] = useState(false)
  const [editError, setEditError] = useState<string | null>(null)

  const load = async () => {
    try {
      const list = await listProviders()
      setProviders(list)
      // 并行加载各 Provider 的模型列表
      const entries = await Promise.all(
        list.map(async (p) => {
          try {
            return [p.id, await listProviderModels(p.id)] as const
          } catch {
            return [p.id, []] as const
          }
        }),
      )
      setModelsMap(Object.fromEntries(entries))
    } catch (e) {
      setError(String(e))
    }
  }

  useEffect(() => { load() }, [])

  // 选择模板：自动填充全部字段（名称按当前语言本地化）
  const applyTemplate = (tpl: ProviderTemplate) => {
    setSelectedTpl(tpl.key)
    setForm({
      name: t(`provider.tpl.${tpl.key}`),
      provider_type: tpl.provider_type,
      protocol: tpl.protocol,
      base_url: tpl.base_url,
      api_key: '',
      endpoints: tpl.endpoints ?? [],
    })
    setModels(tpl.models)
    setKeyHint(tpl.keyHint)
    setModelInput('')
  }

  // Base URL 智能识别：输入后自动匹配模板补全
  const handleUrlBlur = () => {
    if (!form.base_url.trim()) return
    const tpl = matchTemplateByUrl(form.base_url)
    if (tpl && !form.name.trim()) {
      setForm((f) => ({
        ...f,
        name: t(`provider.tpl.${tpl.key}`),
        provider_type: tpl.provider_type,
        protocol: tpl.protocol,
      }))
      setSelectedTpl(tpl.key)
    }
    if (tpl && models.length === 0) {
      setModels(tpl.models)
    }
    if (tpl) setKeyHint(tpl.keyHint)
  }

  // 添加表单：输入框添加模型标签
  const addFormModel = () => {
    const m = modelInput.trim()
    if (!m) return
    if (!models.includes(m)) setModels((prev) => [...prev, m])
    setModelInput('')
  }

  const removeFormModel = (m: string) => {
    setModels((prev) => prev.filter((x) => x !== m))
  }

  const handleCreate = async () => {
    setError(null)
    if (!form.name.trim() || !form.base_url.trim()) {
      setError(t('provider.required'))
      return
    }
    try {
      await createProvider({
        ...form,
        name: form.name.trim(),
        base_url: form.base_url.trim(),
        models: models.map((m) => ({ model_id: m })),
      })
      setForm({ name: '', provider_type: 'openai-compatible', protocol: 'openai', base_url: '', api_key: '', endpoints: [] })
      setModels([])
      setKeyHint('')
      setSelectedTpl(null)
      setShowForm(false)
      load()
    } catch (e) {
      setError(String(e))
    }
  }

  const handleSwitch = async (id: string) => {
    await switchProvider(id)
    load()
  }

  const handleDelete = async (id: string) => {
    if (!confirm(t('provider.deleteConfirm'))) return
    await deleteProvider(id)
    load()
  }

  const handleTest = async (id: string) => {
    setTestResult((prev) => ({ ...prev, [id]: t('provider.testing') }))
    try {
      const result = await testProvider(id)
      setTestResult((prev) => ({ ...prev, [id]: result }))
    } catch (e) {
      setTestResult((prev) => ({ ...prev, [id]: `${t('provider.testFailed')}: ${e}` }))
    }
  }

  // 设为默认模型
  const setDefaultModel = async (m: ProviderModel) => {
    if (m.is_default) return
    try {
      const updated = await updateModel(m.id, { is_default: true })
      const list = (modelsMap[m.provider_id] ?? []).map((x) =>
        x.id === updated.id ? updated : { ...x, is_default: false },
      )
      setModelsMap((prev) => ({ ...prev, [m.provider_id]: list }))
    } catch {
      // 忽略
    }
  }

  // 开始编辑 Provider：加载当前配置与模型
  const startEdit = (p: Provider) => {
    setEditingId(p.id)
    setEditForm({ name: p.name, protocol: p.protocol, base_url: p.base_url, api_key: p.api_key ?? '', endpoints: p.endpoints ?? [] })
    setEditModels(modelsMap[p.id] ?? [])
    setEditModelInput('')
    setEditModPreset('text')
    setEditModIn(['text'])
    setEditModOut(['text'])
    setEditError(null)
  }

  // 保存 Provider 修改（名称/协议/Base URL/API Key）
  const saveEdit = async (id: string) => {
    if (!editForm.name.trim() || !editForm.base_url.trim()) {
      setEditError(t('provider.required'))
      return
    }
    setEditSaving(true)
    try {
      await updateProvider(id, {
        name: editForm.name.trim(),
        base_url: editForm.base_url.trim(),
        api_key: editForm.api_key.trim() || undefined,
        protocol: editForm.protocol,
        endpoints: editForm.endpoints,
      })
      setEditingId(null)
      load()
    } catch (e) {
      setEditError(String(e))
    } finally {
      setEditSaving(false)
    }
  }

  // 编辑面板内添加模型（携带所选类型模态）
  const addEditModel = async () => {
    const m = editModelInput.trim()
    if (!m || !editingId) return
    if (editModels.some((x) => x.model_id === m)) {
      setEditModelInput('')
      return
    }
    try {
      const created = await addModel(editingId, {
        model_id: m,
        input_modalities: editModIn,
        output_modalities: editModOut,
      })
      setEditModels((prev) => [...prev, created])
      setEditModelInput('')
    } catch (e) {
      setEditError(String(e))
    }
  }

  // 模型启用 / 禁用开关
  const toggleEditModel = async (m: ProviderModel) => {
    try {
      const updated = await updateModel(m.id, { enabled: !m.enabled })
      setEditModels((prev) => prev.map((x) => (x.id === updated.id ? updated : x)))
    } catch (e) {
      setEditError(String(e))
    }
  }

  // 模态多选手动勾选：离开预设状态
  const toggleMod = (side: 'in' | 'out', mo: Modality) => {
    setEditModPreset('custom')
    if (side === 'in') {
      setEditModIn((prev) => (prev.includes(mo) ? prev.filter((x) => x !== mo) : [...prev, mo]))
    } else {
      setEditModOut((prev) => (prev.includes(mo) ? prev.filter((x) => x !== mo) : [...prev, mo]))
    }
  }

  // 类型预设一键设置
  const applyModPreset = (key: string) => {
    const p = MODALITY_PRESETS.find((x) => x.key === key)
    if (!p) return
    setEditModPreset(key)
    setEditModIn(p.input)
    setEditModOut(p.output)
  }

  // 编辑面板内删除模型
  const removeEditModel = async (m: ProviderModel) => {
    try {
      await removeModel(m.id)
      setEditModels((prev) => prev.filter((x) => x.id !== m.id))
    } catch (e) {
      setEditError(String(e))
    }
  }

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center justify-between mb-5">
        <div>
          <h2 className="text-xl font-semibold">{t('provider.title')}</h2>
          <p className="text-xs text-[var(--text-secondary)] mt-1">{t('provider.subtitle')}</p>
        </div>
        <button
          onClick={() => setShowForm(!showForm)}
          className="h-9 px-4 rounded-[10px] bg-[var(--accent)] text-white text-[13px] font-medium hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all shadow-lg shadow-[var(--accent)]/15"
        >
          <span className="flex items-center gap-1.5">
            <Icon name={showForm ? 'close' : 'plus'} size={14} white />
            {showForm ? t('provider.cancel') : t('provider.add')}
          </span>
        </button>
      </div>

      {showForm && (
        <div ref={formRef} className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-2xl p-4 mb-6 space-y-4 animate-fade-in-up">
          {/* 模板选择（按分类分组） */}
          <div>
            <div className="text-[11px] font-medium text-[var(--text-muted)] mb-2">{t('provider.templates')}</div>
            <div className="space-y-3">
              {templateCategories.map((cat) => {
                const list = providerTemplates.filter((tpl) => tpl.category === cat.key)
                if (list.length === 0) return null
                return (
                  <div key={cat.key}>
                    <div className="text-[10px] text-[var(--text-muted)] mb-1.5">{t(cat.labelKey)}</div>
                    <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-2">
                      {list.map((tpl) => (
                        <button
                          key={tpl.key}
                          onClick={() => applyTemplate(tpl)}
                          className={`group flex items-center gap-2 px-2.5 py-2 rounded-xl border text-left transition-all ${
                            selectedTpl === tpl.key
                              ? 'border-[var(--accent)] bg-[var(--accent-soft)]'
                              : 'border-[var(--border)] bg-[var(--bg-card)] hover:border-[var(--accent)]/40 hover:bg-[var(--bg-hover)]'
                          }`}
                        >
                          <span
                            className="w-6 h-6 rounded-full flex items-center justify-center shrink-0 text-[10px] font-bold text-white"
                            style={{ backgroundColor: tpl.color }}
                          >
                            {t(`provider.tpl.${tpl.key}`).charAt(0)}
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block text-[12px] truncate text-[var(--text-primary)]">
                              {t(`provider.tpl.${tpl.key}`)}
                            </span>
                            {tpl.free && (
                              <span className="block text-[10px] text-[var(--success)]">{t('provider.freeMark')}</span>
                            )}
                          </span>
                          {selectedTpl === tpl.key && (
                            <Icon name="check" size={13} className="text-[var(--accent)] shrink-0" />
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )
              })}
            </div>
          </div>

          {/* 表单字段 */}
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <label className="text-[11px] text-[var(--text-muted)]">{t('provider.name')}</label>
              <input
                placeholder={t('provider.namePlaceholder')}
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                className="w-full h-9 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-[11px] text-[var(--text-muted)]">{t('provider.baseUrl')}</label>
              <input
                placeholder="https://api.example.com/v1"
                value={form.base_url}
                onChange={(e) => setForm({ ...form, base_url: e.target.value })}
                onBlur={handleUrlBlur}
                className="w-full h-9 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div className="space-y-1.5 sm:col-span-2">
              <label className="text-[11px] text-[var(--text-muted)]">{t('provider.apiKey')}</label>
              <input
                placeholder={keyHint || t('provider.apiKeyPlaceholder')}
                type="password"
                value={form.api_key || ''}
                onChange={(e) => setForm({ ...form, api_key: e.target.value })}
                className="w-full h-9 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
          </div>

          {/* 多协议端点：同一厂商不同协议的 base_url（如 DeepSeek 的 OpenAI/Anthropic 端点） */}
          <EndpointEditor
            endpoints={form.endpoints ?? []}
            onChange={(endpoints) => setForm((f) => ({ ...f, endpoints }))}
          />

          {/* 默认模型 */}
          <div className="space-y-1.5">
            <label className="text-[11px] text-[var(--text-muted)]">{t('provider.models')}</label>
            <div className="flex gap-2 flex-wrap items-center">
              {models.map((m) => (
                <span
                  key={m}
                  className="group flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-[var(--accent-soft)] border border-[var(--accent)]/20 text-[12px] font-mono text-[var(--text-primary)]"
                >
                  {m}
                  <button
                    onClick={() => removeFormModel(m)}
                    className="text-[var(--text-muted)] hover:text-[var(--danger)] transition-colors"
                    title={t('provider.removeModel')}
                  >
                    <Icon name="close" size={11} />
                  </button>
                </span>
              ))}
              <input
                placeholder={t('provider.modelPlaceholder')}
                value={modelInput}
                onChange={(e) => setModelInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ',') {
                    e.preventDefault()
                    addFormModel()
                  }
                }}
                onBlur={addFormModel}
                className="h-8 px-3 flex-1 min-w-40 bg-[var(--bg-card)] border border-dashed border-[var(--border)] rounded-lg text-[12px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <p className="text-[10px] text-[var(--text-muted)]">{t('provider.modelHint')}</p>
          </div>

          <div className="flex items-center gap-3">
            {error && <span className="text-xs text-[var(--danger)]">{error}</span>}
            <button
              onClick={handleCreate}
              className="h-9 px-5 rounded-[10px] bg-[var(--success)] text-white text-[13px] font-medium hover:opacity-90 active:scale-[0.98] transition-all"
            >
              {t('provider.save')}
            </button>
          </div>
        </div>
      )}

      {/* Provider 列表 */}
      <div className="flex-1 overflow-y-auto space-y-2.5 pb-4">
        {providers.length === 0 && !showForm && (
          <div className="flex flex-col items-center gap-3 py-16 text-center">
            <div className="w-14 h-14 rounded-2xl bg-[var(--accent-soft)] flex items-center justify-center">
              <Icon name="bolt" size={24} className="opacity-60" />
            </div>
            <p className="text-[var(--text-secondary)] text-sm">{t('provider.empty')}</p>
            <button
              onClick={() => setShowForm(true)}
              className="h-9 px-4 rounded-lg bg-[var(--accent)] text-white text-[13px] font-medium hover:bg-[var(--accent-hover)] transition-all"
            >
              {t('provider.add')}
            </button>
          </div>
        )}
        {providers.map((p) => (
          <div
            key={p.id}
            className={`bg-[var(--bg-secondary)] border rounded-xl p-4 transition-colors ${
              p.is_active ? 'border-[var(--success)]/40' : 'border-[var(--border)]'
            }`}
          >
            <div className="flex items-start justify-between gap-3">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="font-medium text-[13.5px]">{p.name}</span>
                  {p.is_active && (
                    <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--success)]/15 text-[var(--success)] font-medium">
                      {t('provider.active')}
                    </span>
                  )}
                  <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--bg-card)] text-[var(--text-muted)] font-mono">
                    {p.provider_type}
                  </span>
                  {p.protocol !== 'openai' && (
                    <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--accent-soft)] text-[var(--accent)] font-mono">
                      {p.protocol}
                    </span>
                  )}
                </div>
                <p className="text-xs font-mono text-[var(--text-secondary)] mt-1 break-all">{p.base_url}</p>
                {(p.endpoints?.length ?? 0) > 0 && (
                  <div className="flex gap-1.5 flex-wrap mt-1.5">
                    {p.endpoints.map((ep) => (
                      <span
                        key={`${ep.protocol}-${ep.base_url}`}
                        className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--accent-soft)]/60 text-[var(--text-secondary)] font-mono"
                      >
                        {ep.protocol}: {ep.base_url}
                      </span>
                    ))}
                  </div>
                )}
                {/* 模型徽章：点击★设为默认 */}
                {(modelsMap[p.id]?.length ?? 0) > 0 && (
                  <div className="flex gap-1.5 flex-wrap mt-2">
                    {modelsMap[p.id].map((m) => (
                      <span
                        key={m.id}
                        title={`${m.model_id}${m.enabled ? '' : `（${t('provider.modelDisabled')}）`}`}
                        className={`group/model flex items-center gap-1 text-[10px] px-2 py-0.5 rounded-md font-mono transition-colors ${
                          m.is_default
                            ? 'bg-[var(--accent-soft)] text-[var(--accent)] border border-[var(--accent)]/20'
                            : 'bg-[var(--bg-card)] text-[var(--text-muted)]'
                        } ${!m.enabled ? 'opacity-45' : ''}`}
                      >
                        <button
                          onClick={() => setDefaultModel(m)}
                          title={m.is_default ? t('provider.modelDefault') : t('provider.setDefault')}
                          className={`transition-transform hover:scale-125 ${m.is_default ? '' : 'opacity-30 hover:opacity-100'}`}
                        >
                          ★
                        </button>
                        <span className={!m.enabled ? 'line-through decoration-1' : ''}>{m.model_id}</span>
                        <span className="text-[9px] px-1 rounded bg-[var(--bg-primary)]/80">{modalityShort(m.input_modalities, t)}</span>
                      </span>
                    ))}
                  </div>
                )}
                {testResult[p.id] && (
                  <p className="text-xs mt-1.5 text-[var(--warning)] font-mono">{testResult[p.id]}</p>
                )}
                {/* 编辑面板 */}
                {editingId === p.id && (
                  <div className="mt-3 pt-3 border-t border-[var(--border)] space-y-3 animate-fade-in-up">
                    <p className="text-[11px] text-[var(--text-muted)]">{t('provider.editHint')}</p>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-[var(--text-muted)]">{t('provider.name')}</label>
                        <input
                          value={editForm.name}
                          onChange={(e) => setEditForm({ ...editForm, name: e.target.value })}
                          className="w-full h-8 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-[var(--text-muted)]">{t('provider.protocol')}</label>
                        <select
                          value={editForm.protocol}
                          onChange={(e) => setEditForm({ ...editForm, protocol: e.target.value })}
                          className="w-full h-8 px-2 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
                        >
                          <option value="openai">{t('provider.protoOpenai')}</option>
                          <option value="anthropic">{t('provider.protoAnthropic')}</option>
                          <option value="gemini">{t('provider.protoGemini')}</option>
                        </select>
                      </div>
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-[var(--text-muted)]">{t('provider.baseUrl')}</label>
                        <input
                          value={editForm.base_url}
                          onChange={(e) => setEditForm({ ...editForm, base_url: e.target.value })}
                          placeholder="https://api.example.com/v1"
                          className="w-full h-8 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[12px] font-mono text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-[var(--text-muted)]">{t('provider.apiKey')}</label>
                        <input
                          type="password"
                          value={editForm.api_key}
                          onChange={(e) => setEditForm({ ...editForm, api_key: e.target.value })}
                          placeholder={t('provider.apiKeyPlaceholder')}
                          className="w-full h-8 px-3 bg-[var(--bg-card)] border border-[var(--border)] rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                    </div>
                    {/* 多协议端点 */}
                    <EndpointEditor
                      endpoints={editForm.endpoints}
                      onChange={(endpoints) => setEditForm((f) => ({ ...f, endpoints }))}
                    />
                    {/* 模型管理：chip 含启用开关与类型徽标 */}
                    <div className="space-y-1.5">
                      <label className="text-[11px] text-[var(--text-muted)]">{t('provider.models')}</label>
                      <div className="flex gap-1.5 flex-wrap items-center">
                        {editModels.map((m) => (
                          <span
                            key={m.id}
                            className={`group flex items-center gap-1.5 px-2.5 py-1 rounded-lg border text-[11px] font-mono transition-colors ${
                              m.enabled
                                ? 'bg-[var(--accent-soft)] border-[var(--accent)]/20 text-[var(--text-primary)]'
                                : 'bg-[var(--bg-card)] border-[var(--border)] text-[var(--text-muted)] opacity-60'
                            }`}
                          >
                            {m.is_default && <span className="text-[var(--accent)]">★</span>}
                            <button
                              onClick={() => toggleEditModel(m)}
                              title={m.enabled ? t('provider.modelDisable') : t('provider.modelEnable')}
                              className={`w-3.5 h-3.5 rounded-full border flex items-center justify-center text-[8px] transition-colors ${
                                m.enabled
                                  ? 'border-[var(--success)] text-[var(--success)]'
                                  : 'border-[var(--text-muted)] text-[var(--text-muted)]'
                              } hover:scale-110`}
                            >
                              {m.enabled ? '●' : '○'}
                            </button>
                            <span className={m.enabled ? '' : 'line-through decoration-1'}>{m.model_id}</span>
                            <span className="text-[9px] px-1 rounded bg-[var(--bg-primary)]/80">
                              {modalityShort(m.input_modalities, t)}
                            </span>
                            <button
                              onClick={() => removeEditModel(m)}
                              className="text-[var(--text-muted)] hover:text-[var(--danger)] transition-colors"
                              title={t('provider.removeModel')}
                            >
                              <Icon name="close" size={11} />
                            </button>
                          </span>
                        ))}
                        <input
                          placeholder={t('provider.modelPlaceholder')}
                          value={editModelInput}
                          onChange={(e) => setEditModelInput(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter' || e.key === ',') {
                              e.preventDefault()
                              addEditModel()
                            }
                          }}
                          onBlur={addEditModel}
                          className="h-7 px-3 flex-1 min-w-40 bg-[var(--bg-card)] border border-dashed border-[var(--border)] rounded-lg text-[11px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                      {/* 类型选择：预设单选 + 输入/输出模态多选 */}
                      <ModalityPicker
                        preset={editModPreset}
                        inMods={editModIn}
                        outMods={editModOut}
                        onPreset={applyModPreset}
                        onToggle={toggleMod}
                      />
                    </div>
                    {editError && <p className="text-xs text-[var(--danger)]">{editError}</p>}
                    <div className="flex items-center gap-2">
                      <button
                        onClick={() => saveEdit(p.id)}
                        disabled={editSaving}
                        className="h-8 px-4 text-[12px] bg-[var(--success)] text-white rounded-lg hover:opacity-90 active:scale-[0.98] transition-all disabled:opacity-50"
                      >
                        {editSaving ? t('provider.saving') : t('provider.saveEdit')}
                      </button>
                      <button
                        onClick={() => setEditingId(null)}
                        className="h-8 px-3 text-[12px] border border-[var(--border)] rounded-lg text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
                      >
                        {t('provider.cancel')}
                      </button>
                    </div>
                  </div>
                )}
              </div>
              <div className="flex items-center gap-1.5 shrink-0">
                <button
                  onClick={() => handleTest(p.id)}
                  className="h-7 px-2.5 text-[11px] border border-[var(--border)] rounded-lg text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] transition-colors"
                >
                  {t('provider.test')}
                </button>
                <button
                  onClick={() => (editingId === p.id ? setEditingId(null) : startEdit(p))}
                  className={`h-7 px-2.5 text-[11px] border rounded-lg transition-colors ${
                    editingId === p.id
                      ? 'border-[var(--accent)] text-[var(--accent)]'
                      : 'border-[var(--border)] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  {editingId === p.id ? t('provider.cancel') : t('provider.edit')}
                </button>
                {!p.is_active && (
                  <button
                    onClick={() => handleSwitch(p.id)}
                    className="h-7 px-2.5 text-[11px] bg-[var(--accent)] text-white rounded-lg hover:bg-[var(--accent-hover)] transition-colors"
                  >
                    {t('provider.switch')}
                  </button>
                )}
                <button
                  onClick={() => handleDelete(p.id)}
                  className="h-7 px-2.5 text-[11px] border border-[var(--danger)]/30 text-[var(--danger)] rounded-lg hover:bg-[var(--danger)] hover:text-white transition-colors"
                >
                  {t('provider.delete')}
                </button>
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
