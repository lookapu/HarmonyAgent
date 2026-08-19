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

export interface EmbedStatus {
  /** 语义模型是否可用（未启用 embedding feature 或模型文件缺失均为 false） */
  available: boolean
  model: string | null
  /** 已索引条数 */
  indexed: number
  /** 知识库总条数 */
  total: number
  /** 是否有建索引任务在后台运行 */
  running: boolean
}

export interface EmbedDonePayload {
  ok: boolean
  indexed?: number
  skipped?: number
  elapsed?: number
  error?: string
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
