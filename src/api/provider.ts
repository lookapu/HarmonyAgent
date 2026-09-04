import { invokeWithError } from './invoke'

/** 多协议端点（同一厂商可提供 OpenAI / Anthropic / Gemini 多套端点，如 DeepSeek） */
export interface EndpointDef {
  protocol: string // openai | anthropic | gemini
  base_url: string
}

export interface Provider {
  id: string
  name: string
  provider_type: string
  protocol: string // openai | anthropic | gemini
  base_url: string
  /** 多协议端点（可选：对话按所选协议匹配端点） */
  endpoints: EndpointDef[]
  api_key: string | null
  npm_package: string | null
  is_active: boolean
  in_failover_queue: boolean
  priority: number
  cost_multiplier: number
  limit_daily_cny: number | null
  limit_monthly_cny: number | null
  settings_json: string
  notes: string | null
  icon: string | null
  /** auto 池三态：0=不参与，1=仅主对话，2=主对话+杂活 */
  auto_pool_mode: number
  created_at: number
  updated_at: number
}

export interface CreateProviderInput {
  name: string
  provider_type: string
  protocol?: string // openai | anthropic | gemini
  base_url: string
  /** 多协议端点（可选：如 DeepSeek 同时提供 OpenAI 与 Anthropic 端点） */
  endpoints?: EndpointDef[]
  api_key?: string
  npm_package?: string
  models?: CreateModelInput[]
  notes?: string
}

export interface CreateModelInput {
  model_id: string
  display_name?: string
  tool_call?: boolean
  context_limit?: number
  output_limit?: number
  input_modalities?: string[]
  output_modalities?: string[]
  use_proxy?: boolean // 是否走系统代理
}

export interface ProviderModel {
  id: string
  provider_id: string
  model_id: string
  display_name: string | null
  tool_call: boolean
  context_limit: number
  output_limit: number
  input_modalities: string
  output_modalities: string
  is_default: boolean
  use_proxy: boolean
  enabled: boolean
  created_at: number
  /** 手动排序序号（默认模型强制置顶后，其余按此升序排列） */
  sort_order: number
}

export interface UpdateModelInput {
  use_proxy?: boolean
  is_default?: boolean
  display_name?: string
  context_limit?: number
  output_limit?: number
  enabled?: boolean
  /** 输入模态（text/image/audio/video） */
  input_modalities?: string[]
  /** 输出模态（text/image/audio/video） */
  output_modalities?: string[]
}

export interface UpdateProviderInput {
  name?: string
  base_url?: string
  api_key?: string
  npm_package?: string
  notes?: string
  priority?: number
  protocol?: string // openai | anthropic | gemini
  endpoints?: EndpointDef[]
  limit_daily_cny?: number | null // 日预算（元），0/null 表示不限制
  limit_monthly_cny?: number | null // 月预算（元），0/null 表示不限制
  /** auto 池三态：0=不参与，1=仅主对话，2=主对话+杂活 */
  auto_pool_mode?: number
}

/** 远端模型元数据（同步结果）：平台模型列表的展开信息 */
export interface RemoteModelInfo {
  id: string
  /** 上下文窗口（token）；平台未提供时为 0 */
  context_length: number
  /** 输入价格（美元/百万 token） */
  input_price: number
  /** 输出价格（美元/百万 token） */
  output_price: number
  /** 免费模型（OpenRouter :free 后缀或输入/输出价格均为 0） */
  free: boolean
}

export interface SyncModelsResult {
  provider_id: string
  /** 平台当前返回的模型列表（含元数据，已按免费优先→价格升序→上下文降序排序） */
  remote_models: RemoteModelInfo[]
  /** 本地已配置但平台当前不可用的模型 ID（默认模型等旧配置） */
  missing: string[]
  /** 平台当前有、但本地未配置的模型（新增候选，含元数据） */
  new_models: RemoteModelInfo[]
  /** 拉取远端模型列表失败时的原因（null=成功） */
  error: string | null
}

export const listProviders = () => invokeWithError<Provider[]>('list_providers')
/** 手动排序 Provider：orderedIds 为全部 Provider 的新顺序（含当前激活的），返回排序后的列表 */
export const reorderProviders = (orderedIds: string[]) => invokeWithError<Provider[]>('reorder_providers', { orderedIds })
export const createProvider = (input: CreateProviderInput) => invokeWithError<Provider>('create_provider', { input })
export const updateProvider = (id: string, input: UpdateProviderInput) => invokeWithError<Provider>('update_provider', { id, input })
export const deleteProvider = (id: string) => invokeWithError<void>('delete_provider', { id })
export const switchProvider = (id: string) => invokeWithError<void>('switch_provider', { id })
export const testProvider = (id: string) => invokeWithError<string>('test_provider', { id })
export const listProviderModels = (providerId: string) => invokeWithError<ProviderModel[]>('list_provider_models', { providerId })
export const updateModel = (id: string, input: UpdateModelInput) => invokeWithError<ProviderModel>('update_model', { id, input })
export const addModel = (providerId: string, input: CreateModelInput) =>
  invokeWithError<ProviderModel>('add_model', { providerId, input })
export const removeModel = (id: string) => invokeWithError<void>('remove_model', { id })
/** 手动排序模型：orderedIds 为该 Provider 下模型的新顺序（需包含全部模型 ID），返回排序后的模型列表 */
export const reorderProviderModels = (providerId: string, orderedIds: string[]) =>
  invokeWithError<ProviderModel[]>('reorder_provider_models', { providerId, orderedIds })
export const syncProviderModels = (providerId: string) =>
  invokeWithError<SyncModelsResult>('sync_provider_models', { id: providerId })
