/**
 * pinyin-pro 的类型子集
 *
 * 为什么单独一个文件：`import type` 会被 TS 擦除，所以**类型导入不会把词典拉进
 * 首屏 chunk**。若在 pinyin.ts 里直接 `import { pinyin } from 'pinyin-pro'`，
 * 那个 import 会同时带上库的类型与实现——即使只写 `import type`，
 * 只要同文件里还有一条值导入，TypeScript 的模块解析仍会把 pinyin-pro 记作运行时依赖。
 *
 * 这里把类型定义独立出来，`pinyin.ts` 里只剩 `import('./pinyin-pro')` 动态导入，
 * 从依赖图上彻底断开静态边。
 */

/** pinyin-pro 的选项（只声明本项目用到的字段） */
export interface PinyinOptions {
  /** 'first' 取首字母；缺省为全拼 */
  pattern?: 'first' | 'full' | undefined
  /** 'none' 无声调 */
  toneType?: 'none' | 'num' | 'symbol' | undefined
  /** 'array' 逐字返回 */
  type?: 'string' | 'array' | undefined
  /** 非中文字符的处理方式 */
  nonZh?: 'consecutive' | 'spaced' | 'removed' | undefined
}

/** pinyin() 的返回值形状（type: 'array' 时为 string[]） */
export type PinyinResult = string | string[]
