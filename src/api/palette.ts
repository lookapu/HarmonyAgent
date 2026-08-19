/**
 * 命令面板（Cmd+K）后端命令注册表 API：
 * 后端 command_palette.rs 注册静态命令（导航/动作），前端 CommandPalette 打开时拉取，
 * 与前端动态命令（会话/模型切换）合并后做 fuzzy 搜索。
 */
import { invokeWithError } from './invoke'

/** 后端命令面板条目（静态注册表） */
export interface PaletteEntry {
  /** 唯一标识：nav:<path> 或 action:<name> */
  id: string
  title: string
  subtitle: string
  group: string
  icon: string
}

/** 拉取全部静态命令（导航 + 动作） */
export const listPaletteCommands = () => invokeWithError<PaletteEntry[]>('list_palette_commands')
