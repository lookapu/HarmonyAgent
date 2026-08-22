import { invokeWithError } from './invoke'

/** DevEco CLI（@deveco/deveco-cli，devecocli 命令）探测结果 */
export interface DevecoCliInfo {
  /** 是否可执行（已安装且 --version 跑通） */
  installed: boolean
  /** devecocli --version 输出（如 1.3.0） */
  version: string
  /** 命中的 shim/可执行路径 */
  path: string | null
  /** 未安装/执行失败时的安装与排障指引 */
  install_hint: string
}

/** 探测 devecocli（健康页展示与 MCP 模板创建引导共用） */
export const detectDevecoCli = () => invokeWithError<DevecoCliInfo>('detect_devecocli')
