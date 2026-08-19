import { invokeWithError } from './invoke'

/** 代码符号（组件/类/接口/函数/方法/路由等） */
export interface CodeSymbol {
  /** 符号类型：component / class / interface / function / method / route / struct / enum / decorator */
  kind: string
  name: string
  /** 相对项目根的文件路径 */
  file: string
  /** 1-based 行号 */
  line: number
  /** 所在类/组件（方法归属，顶层为空） */
  parent?: string | null
}

export interface ProjectOutline {
  components: CodeSymbol[]
  pages: string[]
  symbols_count: number
}

/** 全量扫描项目符号（较重，建议首次加载/刷新时调用） */
export const indexProjectSymbols = (projectId: string, root?: string) =>
  invokeWithError<CodeSymbol[]>('index_project_symbols', { projectId, root: root ?? null })

/** 强制刷新项目符号索引（失效内存缓存后重新扫描） */
export const refreshProjectSymbols = (projectId: string, root?: string) =>
  invokeWithError<CodeSymbol[]>('refresh_project_symbols', { projectId, root: root ?? null })

/** 项目大纲：组件、路由页面、符号总数 */
export const projectOutline = (projectId: string, root?: string) =>
  invokeWithError<ProjectOutline>('project_outline', { projectId, root: root ?? null })

/** 按名称/类型检索符号 */
export const searchSymbols = (projectId: string, query: string, kind?: string, root?: string) =>
  invokeWithError<CodeSymbol[]>('search_symbols', { projectId, query, kind: kind ?? null, root: root ?? null })
/** 跨项目检索符号（附项目名，供「全部项目」范围使用） */
export interface CrossProjectSymbol extends CodeSymbol {
  project_name: string
}
export const searchSymbolsAll = (query: string, kind?: string) =>
  invokeWithError<CrossProjectSymbol[]>('search_symbols_all', { query, kind: kind ?? null })

/** 符号索引元信息（符号/文件数量与数据来源） */
export interface SymbolIndexMeta {
  symbols: number
  files: number
  /** 数据来源：disk（磁盘恢复）/ scan（本次会话扫描建立） */
  source: string
  synced_ago_secs: number
}

/** 文件级符号数量（供文件树徽标展示） */
export interface SymbolCount {
  file: string
  count: number
}

/** 后台预热符号索引：磁盘缓存命中 + 增量校正，静默执行不返回符号 */
export const warmupSymbolIndex = (projectId: string, root?: string) =>
  invokeWithError<void>('warmup_symbol_index', { projectId, root: root ?? null })

/** 查询符号索引元信息 */
export const symbolIndexMeta = (projectId: string, root?: string) =>
  invokeWithError<SymbolIndexMeta>('symbol_index_meta', { projectId, root: root ?? null })

/** 查询文件级符号数量（供文件树面板徽标） */
export const symbolCounts = (projectId: string, root?: string) =>
  invokeWithError<SymbolCount[]>('symbol_counts', { projectId, root: root ?? null })
