import { invokeWithError } from './invoke'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// ───────────────────────── 类型 ─────────────────────────

export interface VersionStat {
  version_label: string
  api_level: number | null
  total: number
  added: number
  removed: number
  modified: number
  deprecated: number
}

export interface KitStat {
  kit: string
  total: number
}

export interface ChangeTypeStat {
  change_type: string
  total: number
}

export interface ApiKbStats {
  docs_total: number
  details_total: number
  members_total: number
  versions: VersionStat[]
  kits: KitStat[]
  change_types: ChangeTypeStat[]
  last_refreshed_at: number | null
  last_refreshed_entries: number
}

export interface ApiEntry {
  id: number | null
  kit: string
  dts_file: string | null
  module: string | null
  class_name: string | null
  declaration: string
  api_name: string | null
  change_type: string
  version_label: string
  api_level: number | null
  old_declaration: string | null
  source_url: string
}

export interface DocsPage {
  items: ApiEntry[]
  total: number
  page: number
  page_size: number
}

export interface DocsQuery {
  keyword?: string
  module?: string
  kit?: string
  versionLabel?: string
  apiLevel?: number
  changeType?: string
  page?: number
  pageSize?: number
}

export interface DetailListItem {
  module: string
  slug: string
  title: string | null
  kit: string | null
  since_api_level: number | null
  deprecated: boolean
  has_import: boolean
  has_examples: boolean
  member_count: number
  source_url: string
}

export interface DetailsPage {
  items: DetailListItem[]
  total: number
  page: number
  page_size: number
}

export interface DetailsQuery {
  keyword?: string
  module?: string
  kit?: string
  sinceApiLevel?: number
  includeDeprecated?: boolean
  page?: number
  pageSize?: number
}

export interface ApiMember {
  parent_name: string | null
  member_name: string
  kind: string
  declaration: string | null
  description: string | null
  since_api_level: number | null
  deprecated: boolean
  syscap: string | null
  permission: string | null
}

export interface ApiDetailFull {
  module: string
  slug: string
  title: string | null
  kit: string | null
  since_api_level: number | null
  deprecated: boolean
  import_snippet: string | null
  syscap: string | null
  permissions: string | null
  device_types: string | null
  body: string
  examples: string | null
  source_url: string
  fetched_at: number
  members: ApiMember[]
}

export interface DocInput {
  kit: string
  dtsFile?: string
  module?: string
  className?: string
  declaration: string
  apiName?: string
  changeType: string
  versionLabel: string
  apiLevel?: number
  oldDeclaration?: string
  sourceUrl?: string
}

export interface DetailInput {
  module: string
  title?: string
  kit?: string
  sinceApiLevel?: number
  deprecated?: boolean
  importSnippet?: string
  syscap?: string
  permissions?: string
  deviceTypes?: string
  body: string
  examples?: string
  sourceUrl: string
}

export interface RefreshProgress {
  phase: string
  current: number
  total: number
  message: string
  /** 预计剩余秒数（仅语义索引进度有；样本不足时后端不下发） */
  etaSecs?: number
}

export interface DiffRefreshReport {
  versions_fetched: number
  pages_fetched: number
  entries_inserted: number
  errors: string[]
}

export interface RefRefreshReport {
  pages_fetched: number
  pages_stored: number
  members_stored: number
  errors: string[]
}

export interface KbFilters {
  kits: string[]
  versions: string[]
  modules: string[]
  detail_kits: string[]
}

// ───────────────────────── 语义向量索引 ─────────────────────────

/**
 * 语义层真实状态。
 *
 * **为什么不能只看 `available/indexed/total`**：`indexed === total` 恰恰**盖住**了
 * 「行数满但编码口径已变」这一种 —— 而这正是升级后 / 种子库场景下
 * `vector_search` 正在**拒绝检索**的原因。卡片说就绪、功能是关的，就是这么来的。
 *
 * `partial_index` 是后来加的第五态：建到一半被取消/中断。**只验口径指纹也判不出它**
 * （半成品同样是当前口径产出的），必须数行数——而 `indexed/total` 本来就在返回里，
 * 却没被任何一处用来做判定。
 *
 * `disabled` 是 P-7 方案②加的第六态：用户主动关掉了语义层。
 * 它回答的是「**为什么不做**」，其余五态回答「**为什么不能做**」——
 * 混用就会在关着的时候骗用户去建一个已经存在的索引。
 */
export type EmbedState = 'ready' | 'not_indexed' | 'stale_index' | 'partial_index' | 'model_missing' | 'disabled'

export interface EmbedStatus {
  /**
   * 语义模型是否可用（未启用 embedding feature、模型文件缺失、
   * **或语义层开关已关闭**均为 false）
   */
  available: boolean
  /**
   * 语义层是否参与检索（**默认 false**）。
   * 与 `available` 分开：`available` 答「模型在不在」，`enabled` 答「用户要不要它」。
   */
  enabled: boolean
  model: string | null
  /** 已索引条数 */
  indexed: number
  /** 知识库总条数 */
  total: number
  /** 是否有建索引任务在后台运行 */
  running: boolean
  /** 语义层真实状态（后端 `SemanticLayerState` 的跨语言稳定 key） */
  state: EmbedState
}

export interface EmbedDonePayload {
  ok: boolean
  indexed?: number
  skipped?: number
  elapsed?: number
  error?: string
  /** 用户主动取消（此时 `ok` 仍为 true——已完成的部分是有效结果） */
  cancelled?: boolean
}

// ───────────────────────── 调用封装 ─────────────────────────

export const apiKbStats = () => invokeWithError<ApiKbStats>('api_kb_stats')

export const apiKbFilters = () => invokeWithError<KbFilters>('api_kb_filters')

export const apiDocsList = (query: DocsQuery) =>
  invokeWithError<DocsPage>('api_docs_list', { query })

export const apiDetailsList = (query: DetailsQuery) =>
  invokeWithError<DetailsPage>('api_details_list', { query })

export const apiDetailGet = (slug: string) =>
  invokeWithError<ApiDetailFull>('api_detail_get', { slug })

export const apiDocAdd = (input: DocInput) =>
  invokeWithError<number>('api_doc_add', { input })

export const apiDocDelete = (id: number) =>
  invokeWithError<void>('api_doc_delete', { id })

export const apiDetailUpsert = (input: DetailInput) =>
  invokeWithError<void>('api_detail_upsert', { input })

export const apiDetailDelete = (slug: string) =>
  invokeWithError<void>('api_detail_delete', { slug })

export const apiKbClear = () => invokeWithError<void>('api_kb_clear')

export const apiKbRefreshDocs = () =>
  invokeWithError<DiffRefreshReport>('api_kb_refresh_docs')

export const apiKbRefreshDetails = () =>
  invokeWithError<RefRefreshReport>('api_kb_refresh_details')

export const apiKbEmbedStatus = () => invokeWithError<EmbedStatus>('api_kb_embed_status')

export const apiKbEmbedIndex = () => invokeWithError<void>('api_kb_embed_index')

/** 取消正在运行的语义索引任务（幂等：没在跑也返回成功） */
export const apiKbEmbedCancel = () => invokeWithError<void>('api_kb_embed_cancel')

/**
 * 语义层是否参与检索。**默认 false**（P-7 方案②）。
 * `EmbedStatus.enabled` 已经带了这个值，本函数用于只想读开关、不想拉整份状态的地方。
 */
export const apiKbEmbedEnabled = () => invokeWithError<boolean>('api_kb_embed_enabled')

/** 写语义层开关（幂等）。开启只是允许参与检索，能否真用仍由状态判定决定。 */
export const apiKbEmbedSetEnabled = (enabled: boolean) =>
  invokeWithError<boolean>('api_kb_embed_set_enabled', { enabled })

// ───────────────────────── 进度监听 ─────────────────────────

export function onDocsProgress(cb: (p: RefreshProgress) => void): Promise<UnlistenFn> {
  return listen<RefreshProgress>('api-refresh-progress', (e) => cb(e.payload))
}

export function onDetailsProgress(cb: (p: RefreshProgress) => void): Promise<UnlistenFn> {
  return listen<RefreshProgress>('api-details-progress', (e) => cb(e.payload))
}

export function onEmbedProgress(cb: (p: RefreshProgress) => void): Promise<UnlistenFn> {
  return listen<RefreshProgress>('api-embed-progress', (e) => cb(e.payload))
}

export function onEmbedDone(cb: (p: EmbedDonePayload) => void): Promise<UnlistenFn> {
  return listen<EmbedDonePayload>('api-embed-done', (e) => cb(e.payload))
}
