/**
 * className 拼接
 *
 * 刻意不引 clsx / tailwind-merge：
 * - clsx 只是传递依赖（node_modules 里有但 package.json 未声明），npm ci 后随时可能消失
 * - tailwind-merge 约 20KB，而 main index chunk 只剩 ~40KB 预算余量
 *
 * 不做「同类 utility 后者覆盖前者」的合并，因此约定：src/components/ui/ 里的组件
 * 内部类名永远写在前面，外部传入的 className 永远追加在后面，且只允许附加
 * margin / 宽度 / 定位这类不与内部冲突的类。变体一律走封闭的 variant / size
 * 联合类型，不靠调用方传 Tailwind 类去覆盖。
 *
 * 假值（false / null / undefined / '' / 0）直接丢弃，方便写条件类名：
 *   cn('btn', isDanger && 'btn-danger')
 */

export type ClassValue = string | number | false | null | undefined

export function cn(...values: ClassValue[]): string {
  return values.filter(Boolean).join(' ')
}
