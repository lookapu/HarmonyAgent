// @ui-states: empty, failed, retry
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
  reorderProviderModels,
  reorderProviders,
  syncProviderModels,
  type Provider,
  type CreateProviderInput,
  type ProviderModel,
  type EndpointDef,
  type RemoteModelInfo,
  type SyncModelsResult,
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

/** 视觉模型判定：模板显式标记的 visionModels，或 ID 含 vision/-vl 关键词（自动推断）
 *  命中时添加模型自动带上 image 输入模态，避免用户漏配导致图片发送失败 */
function looksVisionModel(modelId: string): boolean {
  if (providerTemplates.some((t) => t.visionModels?.includes(modelId))) return true
  return /vision/i.test(modelId) || /-vl$/i.test(modelId)
}

/** 生成模型判定：模板 generationModels 命中 → 返回对应输出模态数组（image/video/audio），
 *  未命中返回 undefined（保持默认 text 输出，生成模型不参与对话调度） */
function looksGenerationModel(modelId: string): Modality[] | undefined {
  const out: Modality[] = []
  for (const t of providerTemplates) {
    const g = t.generationModels
    if (g?.image?.models.includes(modelId)) out.push('image')
    if (g?.video?.models.includes(modelId)) out.push('video')
    if (g?.audio?.models.includes(modelId)) out.push('audio')
  }
  return out.length ? [...new Set(out)] : undefined
}

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
            <span className="flex-1 min-w-0 h-8 px-3 modern-card rounded-lg text-[12px] font-mono text-[var(--text-primary)] flex items-center truncate">
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
          className="w-24 h-8 px-2 rounded-lg modern-card border-[var(--border)] text-[11px] font-mono text-[var(--text-primary)] outline-none focus:border-[var(--accent)]"
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
          className="flex-1 min-w-0 h-8 px-3 modern-card rounded-lg text-[12px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
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


/** 模型排序：默认模型永远置顶，其余按手动 sort_order → 创建时间 → id 升序（与后端 ORDER BY 一致） */
function sortModels(list: ProviderModel[]): ProviderModel[] {
  return [...list].sort((a, b) => {
    if (a.is_default !== b.is_default) return a.is_default ? -1 : 1
    if (a.sort_order !== b.sort_order) return a.sort_order - b.sort_order
    if (a.created_at !== b.created_at) return a.created_at - b.created_at
    return a.id < b.id ? -1 : 1
  })
}

/** 上下文窗口格式化：200000 → 200K，1000000 → 1M；0/空 → '—' */
function fmtCtx(n: number | null | undefined): string {
  if (!n || n <= 0) return '—'
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1000) return `${Math.round(n / 1000)}K`
  return String(n)
}

/** 价格格式化（美元/百万 token）：0 → FREE；极小值保留有效位数 */
function fmtPrice(n: number): string {
  if (!n || n <= 0) return 'FREE'
  if (n >= 100) return `$${n.toFixed(0)}`
  if (n >= 1) return `$${n.toFixed(2)}`
  return `$${n.toPrecision(2)}`
}

/** 远端模型排序：免费优先 / 价格升序 / 默认（后端返回顺序） */
function sortRemote(list: RemoteModelInfo[], mode: 'free' | 'price' | 'default'): RemoteModelInfo[] {
  if (mode === 'default') return list
  return [...list].sort((a, b) => {
    if (mode === 'free') {
      if (a.free !== b.free) return a.free ? -1 : 1
    }
    return a.input_price + a.output_price - (b.input_price + b.output_price)
  })
}

/** 解析 token 输入：支持纯数字与 K/M 缩写（200K=200000，1M=1000000）；空/非法返回 undefined（走默认值） */
function parseTokenInput(s: string): number | undefined {
  const v = s.trim()
  if (!v) return undefined
  const m = /^(\d+(?:\.\d+)?)\s*([km])?$/i.exec(v)
  if (!m) return undefined
  const mult = m[2] ? (m[2].toLowerCase() === 'k' ? 1000 : 1_000_000) : 1
  const n = Math.round(parseFloat(m[1]) * mult)
  return Number.isFinite(n) && n > 0 ? n : undefined
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
  // 同步模型配置：syncingId=正在同步的 Provider；syncResults=各 Provider 同步结果；syncBusy=同步面板内正在增删的模型
  const [syncingId, setSyncingId] = useState<string | null>(null)
  const [syncResults, setSyncResults] = useState<Record<string, SyncModelsResult>>({})
  const [syncBusy, setSyncBusy] = useState<Record<string, 'remove' | 'add'>>({})
  // 新添加模型的类型（预设 + 输入/输出模态多选）
  const [editModPreset, setEditModPreset] = useState('text')
  const [editModIn, setEditModIn] = useState<Modality[]>(['text'])
  const [editModOut, setEditModOut] = useState<Modality[]>(['text'])
  // 编辑已有模型的类型（模态）
  const [editTypeModel, setEditTypeModel] = useState<ProviderModel | null>(null)
  const [editTypeIn, setEditTypeIn] = useState<Modality[]>(['text'])
  const [editTypeOut, setEditTypeOut] = useState<Modality[]>(['text'])
  const [editSaving, setEditSaving] = useState(false)
  const [editError, setEditError] = useState<string | null>(null)
  // 同步面板排序/筛选：免费优先（默认） / 价格升序 / 后端顺序；可只看免费
  const [syncSort, setSyncSort] = useState<'free' | 'price' | 'default'>('free')
  const [syncFreeOnly, setSyncFreeOnly] = useState(false)
  // 新添加模型的上下文窗口 / 输出上限（作用于「即将添加的新模型」，与模态选择器同模式）
  const [editCtx, setEditCtx] = useState('')
  const [editOut, setEditOut] = useState('')
  // 已有模型编辑：上下文窗口 / 输出上限（✎ 面板内与模态一起保存）
  const [editTypeCtx, setEditTypeCtx] = useState('')
  const [editTypeOutLimit, setEditTypeOutLimit] = useState('')
  // 创建表单：新建模型的默认上下文窗口 / 输出上限（应用到本次添加的全部模型）
  const [formCtx, setFormCtx] = useState('')
  const [formOut, setFormOut] = useState('')

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
            return [p.id, [] as ProviderModel[]] as const
          }
        }),
      )
      setModelsMap(Object.fromEntries(entries.map(([id, list]) => [id, sortModels(list)])))
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
        models: models.map((m) => ({
          model_id: m,
          input_modalities: looksVisionModel(m) ? ['text', 'image'] : ['text'],
          output_modalities: looksGenerationModel(m) ?? ['text'],
          context_limit: parseTokenInput(formCtx),
          output_limit: parseTokenInput(formOut),
        })),
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

  // 同步 Provider 模型配置：拉取平台当前模型列表，对比本地后展示失效/新增
  const handleSync = async (p: Provider) => {
    if (syncingId) return
    setSyncingId(p.id)
    try {
      const result = await syncProviderModels(p.id)
      setSyncResults((prev) => ({ ...prev, [p.id]: result }))
    } catch (e) {
      setSyncResults((prev) => ({
        ...prev,
        [p.id]: { provider_id: p.id, remote_models: [], missing: [], new_models: [], error: String(e) },
      }))
    } finally {
      setSyncingId(null)
    }
  }

  // 同步面板：按 model_id 从本地 modelsMap 找记录，删除平台已失效的模型
  const removeSyncMissing = async (p: Provider, modelId: string) => {
    const local = (modelsMap[p.id] ?? []).find((m) => m.model_id === modelId)
    if (!local) return
    setSyncBusy((prev) => ({ ...prev, [modelId]: 'remove' }))
    try {
      await removeModel(local.id)
      setSyncResults((prev) => {
        const cur = prev[p.id]
        if (!cur) return prev
        return { ...prev, [p.id]: { ...cur, missing: cur.missing.filter((x) => x !== modelId) } }
      })
      await load()
    } catch {
      // 忽略：前端从界面移除失效项
      setSyncResults((prev) => {
        const cur = prev[p.id]
        if (!cur) return prev
        return { ...prev, [p.id]: { ...cur, missing: cur.missing.filter((x) => x !== modelId) } }
      })
      await load()
    } finally {
      setSyncBusy((prev) => {
        const next = { ...prev }
        delete next[modelId]
        return next
      })
    }
  }

  // 批量移除全部失效模型
  const removeAllMissing = async (p: Provider) => {
    const cur = syncResults[p.id]
    if (!cur || cur.missing.length === 0) return
    setSyncBusy((prev) => ({ ...prev, ['__all_' + p.id]: 'remove' }))
    try {
      for (const modelId of cur.missing) {
        const local = (modelsMap[p.id] ?? []).find((m) => m.model_id === modelId)
        if (local) {
          try {
            await removeModel(local.id)
          } catch {
            // 单个删除失败继续删其余
          }
        }
      }
      setSyncResults((prev) => {
        const r = prev[p.id]
        if (!r) return prev
        return { ...prev, [p.id]: { ...r, missing: [] } }
      })
      await load()
    } finally {
      setSyncBusy((prev) => {
        const next = { ...prev }
        delete next['__all_' + p.id]
        return next
      })
    }
  }

  // 同步面板：一键添加平台新增模型（携带远端元数据：上下文窗口自动填充，价格类字段不入库）
  const addSyncModel = async (p: Provider, modelInfo: RemoteModelInfo) => {
    setSyncBusy((prev) => ({ ...prev, [modelInfo.id]: 'add' }))
    try {
      await addModel(p.id, {
        model_id: modelInfo.id,
        input_modalities: looksVisionModel(modelInfo.id) ? ['text', 'image'] : ['text'],
        output_modalities: looksGenerationModel(modelInfo.id) ?? ['text'],
        context_limit: modelInfo.context_length > 0 ? modelInfo.context_length : undefined,
      })
      setSyncResults((prev) => {
        const cur = prev[p.id]
        if (!cur) return prev
        return { ...prev, [p.id]: { ...cur, new_models: cur.new_models.filter((x) => x.id !== modelInfo.id) } }
      })
      await load()
    } catch (e) {
      setSyncResults((prev) => ({
        ...prev,
        [p.id]: {
          provider_id: p.id,
          remote_models: [],
          missing: [],
          new_models: [],
          error: String(e),
        },
      }))
    } finally {
      setSyncBusy((prev) => {
        const next = { ...prev }
        delete next[modelInfo.id]
        return next
      })
    }
  }

  // 批量添加全部新增模型
  const addAllNew = async (p: Provider) => {
    const cur = syncResults[p.id]
    if (!cur || cur.new_models.length === 0) return
    setSyncBusy((prev) => ({ ...prev, ['__all_' + p.id]: 'add' }))
    try {
      for (const modelInfo of cur.new_models) {
        try {
          await addModel(p.id, {
            model_id: modelInfo.id,
            input_modalities: looksVisionModel(modelInfo.id) ? ['text', 'image'] : ['text'],
            output_modalities: looksGenerationModel(modelInfo.id) ?? ['text'],
            context_limit: modelInfo.context_length > 0 ? modelInfo.context_length : undefined,
          })
        } catch {
          // 单个添加失败继续加其余
        }
      }
      setSyncResults((prev) => {
        const r = prev[p.id]
        if (!r) return prev
        return { ...prev, [p.id]: { ...r, new_models: [] } }
      })
      await load()
    } finally {
      setSyncBusy((prev) => {
        const next = { ...prev }
        delete next['__all_' + p.id]
        return next
      })
    }
  }

  // 设为默认模型
  const setDefaultModel = async (m: ProviderModel) => {
    if (m.is_default) return
    try {
      const updated = await updateModel(m.id, { is_default: true })
      const list = sortModels(
        (modelsMap[m.provider_id] ?? []).map((x) =>
          x.id === updated.id ? updated : { ...x, is_default: false },
        ),
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
    setEditCtx('')
    setEditOut('')
    setEditTypeCtx('')
    setEditTypeOutLimit('')
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

  // 编辑面板内添加模型（携带所选类型模态 + 上下文窗口/输出上限）
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
        context_limit: parseTokenInput(editCtx),
        output_limit: parseTokenInput(editOut),
      })
      setEditModels((prev) => sortModels([...prev, created]))
      setEditModelInput('')
    } catch (e) {
      setEditError(String(e))
    }
  }

  // 模型启用 / 禁用开关
  const toggleEditModel = async (m: ProviderModel) => {
    try {
      const updated = await updateModel(m.id, { enabled: !m.enabled })
      setEditModels((prev) => sortModels(prev.map((x) => (x.id === updated.id ? updated : x))))
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

  // 打开已有模型的类型编辑（预填当前模态与上下文/输出上限）
  const startEditType = (m: ProviderModel) => {
    setEditTypeModel(m)
    setEditTypeIn(parseModalities(m.input_modalities))
    setEditTypeOut(parseModalities(m.output_modalities))
    // 预填完整 token 数（如 200000），单位说明在 label/placeholder；仍兼容 K/M 缩写输入
    setEditTypeCtx(m.context_limit > 0 ? String(m.context_limit) : '')
    setEditTypeOutLimit(m.output_limit > 0 ? String(m.output_limit) : '')
  }
  const toggleTypeMod = (side: 'in' | 'out', mo: Modality) => {
    if (side === 'in') {
      setEditTypeIn((prev) => (prev.includes(mo) ? prev.filter((x) => x !== mo) : [...prev, mo]))
    } else {
      setEditTypeOut((prev) => (prev.includes(mo) ? prev.filter((x) => x !== mo) : [...prev, mo]))
    }
  }
  const applyTypePreset = (key: string) => {
    const p = MODALITY_PRESETS.find((x) => x.key === key)
    if (!p) return
    setEditTypeIn(p.input)
    setEditTypeOut(p.output)
  }
  // 当前模态组合命中的预设（用于高亮；不匹配任何预设时为 custom）
  const typePresetKey =
    MODALITY_PRESETS.find(
      (p) =>
        [...p.input].sort().join() === [...editTypeIn].sort().join() &&
        [...p.output].sort().join() === [...editTypeOut].sort().join(),
    )?.key ?? 'custom'
  // 保存已有模型的类型（模态 + 上下文窗口 + 输出上限）
  const saveEditType = async () => {
    if (!editTypeModel) return
    try {
      const updated = await updateModel(editTypeModel.id, {
        input_modalities: editTypeIn,
        output_modalities: editTypeOut,
        context_limit: parseTokenInput(editTypeCtx),
        output_limit: parseTokenInput(editTypeOutLimit),
      })
      setEditModels((prev) => sortModels(prev.map((x) => (x.id === updated.id ? updated : x))))
      setEditTypeModel(null)
    } catch (e) {
      setEditError(String(e))
    }
  }

  // 编辑面板内删除模型；删除默认模型后后端自动顺延下一个为默认，本地同步标记
  const removeEditModel = async (m: ProviderModel) => {
    try {
      await removeModel(m.id)
      setEditModels((prev) => {
        const list = sortModels(prev.filter((x) => x.id !== m.id))
        if (m.is_default && list.length > 0 && !list[0].is_default) {
          list[0] = { ...list[0], is_default: true }
        }
        return list
      })
    } catch (e) {
      setEditError(String(e))
    }
  }

  // 手动排序：上移 / 下移（默认模型置顶不可移动）
  const moveEditModel = async (m: ProviderModel, dir: -1 | 1) => {
    if (!editingId) return
    const ordered = sortModels(editModels).map((x) => x.id)
    const idx = ordered.indexOf(m.id)
    const j = idx + dir
    if (idx < 0 || j < 0 || j >= ordered.length) return
    ;[ordered[idx], ordered[j]] = [ordered[j], ordered[idx]]
    try {
      const updated = await reorderProviderModels(editingId, ordered)
      setEditModels(sortModels(updated))
      // 排序即时生效（无需保存），同步主列表避免关闭编辑后顺序陈旧
      setModelsMap((prev) => ({ ...prev, [editingId]: updated }))
    } catch (e) {
      setEditError(String(e))
    }
  }

  // 主列表手动排序：上移 / 下移（默认模型置顶不可移动），即时生效
  const moveMainModel = async (p: Provider, m: ProviderModel, dir: -1 | 1) => {
    const ordered = sortModels(modelsMap[p.id] ?? []).map((x) => x.id)
    const idx = ordered.indexOf(m.id)
    const j = idx + dir
    if (idx < 0 || j < 0 || j >= ordered.length) return
    ;[ordered[idx], ordered[j]] = [ordered[j], ordered[idx]]
    try {
      const updated = await reorderProviderModels(p.id, ordered)
      setModelsMap((prev) => ({ ...prev, [p.id]: updated }))
    } catch {
      // 忽略：下一次 load() 会纠正
    }
  }

  // Provider 手动排序：上移 / 下移（当前使用的 Provider 置顶，不可移动），即时生效
  const moveProvider = async (p: Provider, dir: -1 | 1) => {
    const ordered = providers.map((x) => x.id)
    const idx = ordered.indexOf(p.id)
    const j = idx + dir
    if (idx < 0 || j < 0 || j >= ordered.length) return
    ;[ordered[idx], ordered[j]] = [ordered[j], ordered[idx]]
    try {
      const updated = await reorderProviders(ordered)
      setProviders(updated)
    } catch {
      // 忽略：下一次 load() 会纠正
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
          className="h-9 px-4 rounded-[10px] btn-primary text-[13px] font-medium active:scale-[0.98] transition-all"
        >
          <span className="flex items-center gap-1.5">
            <Icon name={showForm ? 'close' : 'plus'} size={14} white />
            {showForm ? t('provider.cancel') : t('provider.add')}
          </span>
        </button>
      </div>

      {showForm && (
        <div ref={formRef} className="modern-card rounded-2xl p-4 mb-6 space-y-4 animate-fade-in-up">
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
                className="w-full h-9 px-3 modern-card rounded-lg text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-[11px] text-[var(--text-muted)]">{t('provider.baseUrl')}</label>
              <input
                placeholder="https://api.example.com/v1"
                value={form.base_url}
                onChange={(e) => setForm({ ...form, base_url: e.target.value })}
                onBlur={handleUrlBlur}
                className="w-full h-9 px-3 modern-card rounded-lg text-[13px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div className="space-y-1.5 sm:col-span-2">
              <label className="text-[11px] text-[var(--text-muted)]">{t('provider.apiKey')}</label>
              <input
                placeholder={keyHint || t('provider.apiKeyPlaceholder')}
                type="password"
                value={form.api_key || ''}
                onChange={(e) => setForm({ ...form, api_key: e.target.value })}
                className="w-full h-9 px-3 modern-card rounded-lg text-[13px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
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
                className="h-8 px-3 flex-1 min-w-40 modern-card border-dashed border-[var(--border)] rounded-lg text-[12px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            {/* 新建模型的默认上下文窗口 / 输出上限（应用到本次添加的全部模型；创建后可在编辑面板逐模型调整） */}
            <div className="flex items-center gap-3 flex-wrap">
              <label className="text-[10px] text-[var(--text-muted)]">{t('provider.modelDefaults')}</label>
              <input
                type="text"
                inputMode="numeric"
                placeholder={t('provider.modelCtxPh')}
                value={formCtx}
                onChange={(e) => setFormCtx(e.target.value)}
                className="w-32 h-7 px-2 modern-card rounded-lg text-[11px] tabular-nums text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
              <input
                type="text"
                inputMode="numeric"
                placeholder={t('provider.modelOutPh')}
                value={formOut}
                onChange={(e) => setFormOut(e.target.value)}
                className="w-32 h-7 px-2 modern-card rounded-lg text-[11px] tabular-nums text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </div>
            <p className="text-[10px] text-[var(--text-muted)]">{t('provider.modelHint')}</p>
            <p className="text-[10px] text-[var(--text-muted)]">{t('provider.modelTokenHint')}</p>
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
              className="h-9 px-4 rounded-lg btn-primary text-[13px] font-medium transition-all"
            >
              {t('provider.add')}
            </button>
          </div>
        )}
        {providers.map((p, pIdx) => (
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
                {/* 模型徽章：点击★设为默认，▲/▼手动排序（默认模型置顶不可移动） */}
                {(modelsMap[p.id]?.length ?? 0) > 0 && (
                  <div className="flex gap-1.5 flex-wrap mt-2">
                    {sortModels(modelsMap[p.id] ?? []).map((m, idx, arr) => (
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
                        {m.context_limit > 0 && (
                          <span
                            className="text-[9px] px-1 rounded bg-[var(--bg-primary)]/80 tnum"
                            title={`${t('provider.modelCtx')}: ${m.context_limit}`}
                          >
                            {fmtCtx(m.context_limit)}
                          </span>
                        )}
                        <span className="flex flex-col gap-px opacity-0 group-hover/model:opacity-100 transition-opacity">
                          <button
                            onClick={() => moveMainModel(p, m, -1)}
                            disabled={m.is_default || idx === 0}
                            title={t('provider.modelUp')}
                            className="text-[8px] leading-none text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-20 disabled:hover:text-[var(--text-muted)] transition-colors"
                          >
                            ▲
                          </button>
                          <button
                            onClick={() => moveMainModel(p, m, 1)}
                            disabled={m.is_default || idx === arr.length - 1}
                            title={t('provider.modelDown')}
                            className="text-[8px] leading-none text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-20 disabled:hover:text-[var(--text-muted)] transition-colors"
                          >
                            ▼
                          </button>
                        </span>
                      </span>
                    ))}
                  </div>
                )}
                {testResult[p.id] && (
                  <p className="text-xs mt-1.5 text-[var(--warning)] font-mono">{testResult[p.id]}</p>
                )}
                {/* 同步结果面板：平台失效（可移除）/ 新增（可添加） */}
                {syncResults[p.id] && (
                  <div className="mt-2.5 rounded-xl border border-[var(--border)] bg-[var(--bg-card)]/60 px-3 py-2.5 space-y-2 animate-fade-in-up">
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-[11px] font-medium text-[var(--text-secondary)]">
                        {syncResults[p.id].error
                          ? t('provider.syncFailed')
                          : t('provider.syncDone')}
                      </span>
                      {syncResults[p.id].error && (
                        <button
                          onClick={() => handleSync(p)}
                          disabled={syncingId !== null}
                          className="text-[10px] px-2 py-0.5 border border-[var(--border)] rounded-md text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent)] transition-colors"
                        >
                          {t('provider.syncRetry')}
                        </button>
                      )}
                    </div>
                    {syncResults[p.id].error ? (
                      <p className="text-[10.5px] text-[var(--danger)] font-mono break-all">
                        {t('provider.syncError', { error: syncResults[p.id].error })}
                      </p>
                    ) : (
                      <>
                        <p className="text-[10.5px] text-[var(--text-muted)]">
                          {t('provider.syncSummary', {
                            remote: syncResults[p.id].remote_models.length,
                            local: modelsMap[p.id]?.length ?? 0,
                          })}
                        </p>
                        {/* 排序/筛选：免费优先（默认）/ 价格升序 / 平台顺序；可只看免费 */}
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="text-[10px] text-[var(--text-muted)]">{t('provider.syncSort')}</span>
                          <div className="flex rounded-md border border-[var(--border)] overflow-hidden">
                            {(
                              [
                                ['free', t('provider.syncSortFree')],
                                ['price', t('provider.syncSortPrice')],
                                ['default', t('provider.syncSortDefault')],
                              ] as const
                            ).map(([key, label]) => (
                              <button
                                key={key}
                                onClick={() => setSyncSort(key)}
                                className={`px-2 py-0.5 text-[10px] transition-colors ${
                                  syncSort === key
                                    ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                                    : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                                }`}
                              >
                                {label}
                              </button>
                            ))}
                          </div>
                          <label className="flex items-center gap-1 text-[10px] text-[var(--text-muted)] cursor-pointer select-none">
                            <input
                              type="checkbox"
                              checked={syncFreeOnly}
                              onChange={(e) => setSyncFreeOnly(e.target.checked)}
                              className="w-3 h-3 accent-[var(--accent)]"
                            />
                            {t('provider.syncFreeOnly')}
                          </label>
                        </div>
                        {/* 失效模型：默认模型等旧配置，可移除 */}
                        <div className="space-y-1">
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-[10.5px] text-[var(--warning)]">
                              {t('provider.syncMissing')}（{syncResults[p.id].missing.length}）
                            </span>
                            {syncResults[p.id].missing.length > 0 && (
                              <button
                                onClick={() => removeAllMissing(p)}
                                disabled={syncBusy['__all_' + p.id] === 'remove'}
                                className="text-[10px] px-2 py-0.5 border border-[var(--danger)]/30 text-[var(--danger)] rounded-md hover:bg-[var(--danger)] hover:text-white transition-colors disabled:opacity-50"
                              >
                                {syncBusy['__all_' + p.id] === 'remove' ? t('provider.syncBusy') : t('provider.syncRemoveAll')}
                              </button>
                            )}
                          </div>
                          {syncResults[p.id].missing.length === 0 ? (
                            <p className="text-[10.5px] text-[var(--text-muted)] italic">{t('provider.syncMissingEmpty')}</p>
                          ) : (
                            <div className="flex gap-1.5 flex-wrap">
                              {syncResults[p.id].missing.map((modelId) => (
                                <span
                                  key={modelId}
                                  className="group flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-[var(--danger)]/10 border border-[var(--danger)]/20 text-[10.5px] font-mono text-[var(--danger)]"
                                >
                                  <span className="line-through decoration-1">{modelId}</span>
                                  <button
                                    onClick={() => removeSyncMissing(p, modelId)}
                                    disabled={syncBusy[modelId] === 'remove'}
                                    className="opacity-50 hover:opacity-100 transition-opacity disabled:opacity-30"
                                    title={t('provider.syncRemove')}
                                  >
                                    {syncBusy[modelId] === 'remove' ? t('provider.syncBusy') : <Icon name="close" size={10} />}
                                  </button>
                                </span>
                              ))}
                            </div>
                          )}
                        </div>
                        {/* 新增模型：平台有、本地未配置 */}
                        <div className="space-y-1">
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-[10.5px] text-[var(--success)]">
                              {t('provider.syncNew')}（
                              {sortRemote(syncResults[p.id].new_models, syncSort).filter((m) => !syncFreeOnly || m.free).length}
                              ）
                            </span>
                            {sortRemote(syncResults[p.id].new_models, syncSort).filter((m) => !syncFreeOnly || m.free).length > 0 && (
                              <button
                                onClick={() => addAllNew(p)}
                                disabled={syncBusy['__all_' + p.id] === 'add'}
                                className="text-[10px] px-2 py-0.5 border border-[var(--success)]/30 text-[var(--success)] rounded-md hover:bg-[var(--success)] hover:text-white transition-colors disabled:opacity-50"
                              >
                                {syncBusy['__all_' + p.id] === 'add' ? t('provider.syncBusy') : t('provider.syncAddAll')}
                              </button>
                            )}
                          </div>
                          {sortRemote(syncResults[p.id].new_models, syncSort).filter((m) => !syncFreeOnly || m.free).length === 0 ? (
                            <p className="text-[10.5px] text-[var(--text-muted)] italic">{t('provider.syncNewEmpty')}</p>
                          ) : (
                            <div className="flex gap-1.5 flex-wrap max-h-24 overflow-y-auto">
                              {sortRemote(syncResults[p.id].new_models, syncSort)
                                .filter((m) => !syncFreeOnly || m.free)
                                .map((m) => (
                                  <span
                                    key={m.id}
                                    title={`${m.id} · ${t('provider.modelCtx')} ${fmtCtx(m.context_length)} · ${fmtPrice(m.input_price)}/in · ${fmtPrice(m.output_price)}/out`}
                                    className="group flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-[var(--success)]/10 border border-[var(--success)]/20 text-[10.5px] font-mono text-[var(--success)]"
                                  >
                                    {m.id}
                                    {m.free && (
                                      <span className="text-[8px] px-1 py-px rounded bg-[var(--success)] text-white font-bold leading-none">
                                        FREE
                                      </span>
                                    )}
                                    <span className="text-[9px] text-[var(--text-muted)]/70">{fmtCtx(m.context_length)}</span>
                                    <button
                                      onClick={() => addSyncModel(p, m)}
                                      disabled={syncBusy[m.id] === 'add'}
                                      className="opacity-50 hover:opacity-100 transition-opacity disabled:opacity-30"
                                      title={t('provider.syncAdd')}
                                    >
                                      {syncBusy[m.id] === 'add' ? t('provider.syncBusy') : <Icon name="plus" size={10} />}
                                    </button>
                                  </span>
                                ))}
                            </div>
                          )}
                        </div>
                      </>
                    )}
                  </div>
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
                          className="w-full h-8 px-3 modern-card rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-[var(--text-muted)]">{t('provider.protocol')}</label>
                        <select
                          value={editForm.protocol}
                          onChange={(e) => setEditForm({ ...editForm, protocol: e.target.value })}
                          className="w-full h-8 px-2 modern-card rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
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
                          className="w-full h-8 px-3 modern-card rounded-lg text-[12px] font-mono text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-[var(--text-muted)]">{t('provider.apiKey')}</label>
                        <input
                          type="password"
                          value={editForm.api_key}
                          onChange={(e) => setEditForm({ ...editForm, api_key: e.target.value })}
                          placeholder={t('provider.apiKeyPlaceholder')}
                          className="w-full h-8 px-3 modern-card rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
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
                        {sortModels(editModels).map((m, idx, arr) => (
                          <span
                            key={m.id}
                            className={`group flex items-center gap-1.5 px-2.5 py-1 rounded-lg border text-[11px] font-mono transition-colors ${
                              m.enabled
                                ? 'bg-[var(--accent-soft)] border-[var(--accent)]/20 text-[var(--text-primary)]'
                                : 'modern-card-[var(--border)] text-[var(--text-muted)] opacity-60'
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
                            {m.context_limit > 0 && (
                              <span
                                className="text-[9px] px-1 rounded bg-[var(--bg-primary)]/80 tnum"
                                title={`${t('provider.modelCtx')}: ${m.context_limit}`}
                              >
                                {fmtCtx(m.context_limit)}
                              </span>
                            )}
                            <button
                              onClick={() => startEditType(m)}
                              title={t('provider.modelEditType')}
                              className={`transition-colors ${editTypeModel?.id === m.id ? 'text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)]'}`}
                            >
                              ✎
                            </button>
                            {/* 手动排序：默认模型置顶，不可移动 */}
                            <span className="flex flex-col gap-px">
                              <button
                                onClick={() => moveEditModel(m, -1)}
                                disabled={m.is_default || idx === 0}
                                title={t('provider.modelUp')}
                                className="text-[9px] leading-none text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-20 disabled:hover:text-[var(--text-muted)] transition-colors"
                              >
                                ▲
                              </button>
                              <button
                                onClick={() => moveEditModel(m, 1)}
                                disabled={m.is_default || idx === arr.length - 1}
                                title={t('provider.modelDown')}
                                className="text-[9px] leading-none text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-20 disabled:hover:text-[var(--text-muted)] transition-colors"
                              >
                                ▼
                              </button>
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
                          className="h-7 px-3 flex-1 min-w-40 modern-card border-dashed border-[var(--border)] rounded-lg text-[11px] font-mono text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                      {/* 类型选择：预设单选 + 输入/输出模态多选（仅作用于「即将添加的新模型」，先选类型再按 Enter 添加） */}
                      <ModalityPicker
                        preset={editModPreset}
                        inMods={editModIn}
                        outMods={editModOut}
                        onPreset={applyModPreset}
                        onToggle={toggleMod}
                      />
                      {/* 新建模型的上下文窗口 / 输出上限（与模态选择器同样只作用于「即将添加的新模型」） */}
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="text-[10px] text-[var(--text-muted)]">{t('provider.modelDefaults')}</span>
                        <input
                          type="text"
                          inputMode="numeric"
                          placeholder={t('provider.modelCtxPh')}
                          value={editCtx}
                          onChange={(e) => setEditCtx(e.target.value)}
                          className="w-28 h-7 px-2 modern-card rounded-lg text-[11px] tabular-nums text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
                        />
                        <input
                          type="text"
                          inputMode="numeric"
                          placeholder={t('provider.modelOutPh')}
                          value={editOut}
                          onChange={(e) => setEditOut(e.target.value)}
                          className="w-28 h-7 px-2 modern-card rounded-lg text-[11px] tabular-nums text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
                        />
                      </div>
                      <p className="text-[10px] text-[var(--text-muted)]">{t('provider.modelTokenHint')}</p>
                      {/* 已有模型的类型编辑：点击 chip 上的 ✎ 打开 */}
                      {editTypeModel && (
                        <div className="rounded-xl border border-[var(--accent)]/30 bg-[var(--bg-card)]/60 px-3 py-2.5 space-y-2 animate-fade-in-up">
                          <div className="text-[10.5px] font-medium text-[var(--accent)]">
                            {t('provider.modelEditType')}: {editTypeModel.model_id}
                          </div>
                          <ModalityPicker
                            preset={typePresetKey}
                            inMods={editTypeIn}
                            outMods={editTypeOut}
                            onPreset={applyTypePreset}
                            onToggle={toggleTypeMod}
                          />
                          {/* 已有模型的上下文窗口 / 输出上限（随类型一起保存） */}
                          <div className="flex items-center gap-2 flex-wrap">
                            <span className="text-[10px] text-[var(--text-muted)]">{t('provider.modelCtx')}</span>
                            <input
                              type="text"
                              inputMode="numeric"
                              placeholder={t('provider.modelCtxPh')}
                              value={editTypeCtx}
                              onChange={(e) => setEditTypeCtx(e.target.value)}
                              className="w-28 h-7 px-2 modern-card rounded-lg text-[11px] tabular-nums text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
                            />
                            <span className="text-[10px] text-[var(--text-muted)]">{t('provider.modelOut')}</span>
                            <input
                              type="text"
                              inputMode="numeric"
                              placeholder={t('provider.modelOutPh')}
                              value={editTypeOutLimit}
                              onChange={(e) => setEditTypeOutLimit(e.target.value)}
                              className="w-28 h-7 px-2 modern-card rounded-lg text-[11px] tabular-nums text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
                            />
                          </div>
                          <p className="text-[10px] text-[var(--text-muted)]">{t('provider.modelTokenHint')}</p>
                          <div className="flex items-center gap-2">
                            <button
                              onClick={saveEditType}
                              className="h-7 px-3 text-[11px] bg-[var(--accent)] text-white rounded-lg hover:opacity-90 active:scale-[0.98] transition-all"
                            >
                              {t('provider.typeSave')}
                            </button>
                            <button
                              onClick={() => setEditTypeModel(null)}
                              className="h-7 px-3 text-[11px] border border-[var(--border)] rounded-lg text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
                            >
                              {t('provider.cancel')}
                            </button>
                          </div>
                        </div>
                      )}
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
                {/* Provider 手动排序：当前使用的置顶，不可移动 */}
                <span className="flex flex-col gap-px mr-0.5">
                  <button
                    onClick={() => moveProvider(p, -1)}
                    disabled={p.is_active || pIdx === 0}
                    title={t('provider.providerUp')}
                    className="text-[9px] leading-none text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-20 disabled:hover:text-[var(--text-muted)] transition-colors"
                  >
                    ▲
                  </button>
                  <button
                    onClick={() => moveProvider(p, 1)}
                    disabled={p.is_active || pIdx === providers.length - 1}
                    title={t('provider.providerDown')}
                    className="text-[9px] leading-none text-[var(--text-muted)] hover:text-[var(--accent)] disabled:opacity-20 disabled:hover:text-[var(--text-muted)] transition-colors"
                  >
                    ▼
                  </button>
                </span>
                <button
                  onClick={() => handleSync(p)}
                  disabled={syncingId !== null}
                  title={t('provider.sync')}
                  className={`h-7 px-2.5 text-[11px] border rounded-lg transition-colors ${
                    syncingId === p.id
                      ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)] animate-pulse'
                      : 'border-[var(--border)] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                  } disabled:opacity-50`}
                >
                  {syncingId === p.id ? (
                    <span className="flex items-center gap-1">
                      <Icon name="refresh" size={12} className="animate-spin" />
                      {t('provider.syncing')}
                    </span>
                  ) : (
                    <span className="flex items-center gap-1">
                      <Icon name="refresh" size={12} />
                      {t('provider.sync')}
                    </span>
                  )}
                </button>
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
                    className="h-7 px-2.5 text-[11px] btn-primary rounded-lg transition-colors"
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



