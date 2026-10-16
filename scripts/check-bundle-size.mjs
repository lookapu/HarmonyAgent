import { readdir, stat } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const assetsDir = fileURLToPath(new URL('../dist/assets/', import.meta.url))

/**
 * 预算 = 「当前值 + 一点点余量」，不是随手拍的整数。
 *
 * 下调依据（2026-10-01，Home 瘦身批次）：
 *   Home 703.0KB → **417.1KB**。做法是把 pinyin-pro 的 4 本词典
 *   （打包后 286KB）从静态 import 改为懒加载——它们只在用户打 @ 之后
 *   或打开命令面板时才用得上，首屏必然用不到。
 *   预算随之从 750KB 下调到 **440KB**，留约 5% 余量。
 *
 * 为什么必须下调：预算不收紧，这次瘦身就只是「数字变好看了」，
 * 下一次加回一个重型依赖照样绿灯。**收紧门禁才是验收，不是一次性的测量。**
 */
const budgets = [
  { label: 'Home', pattern: /^Home-.*\.js$/, maxBytes: 440_000 },
  { label: 'Markdown', pattern: /^Markdown-.*\.js$/, maxBytes: 1_550_000 },
  { label: 'main index', pattern: /^index-.*\.js$/, maxBytes: 575_000 },
]
const maxAnyChunkBytes = 1_600_000

const files = (await readdir(assetsDir)).filter((name) => name.endsWith('.js'))
const sizes = new Map(
  await Promise.all(files.map(async (name) => [name, (await stat(join(assetsDir, name))).size])),
)
const failures = []

for (const budget of budgets) {
  const matches = [...sizes].filter(([name]) => budget.pattern.test(name))
  if (matches.length !== 1) {
    failures.push(`${budget.label}: expected exactly one matching chunk, found ${matches.length}`)
    continue
  }
  const [name, bytes] = matches[0]
  console.log(`${budget.label}: ${name} ${(bytes / 1000).toFixed(1)}KB / ${(budget.maxBytes / 1000).toFixed(0)}KB`)
  if (bytes > budget.maxBytes) failures.push(`${name}: ${bytes} bytes exceeds ${budget.maxBytes}`)
}

for (const [name, bytes] of sizes) {
  if (bytes > maxAnyChunkBytes) failures.push(`${name}: ${bytes} bytes exceeds global chunk limit ${maxAnyChunkBytes}`)
}

if (failures.length) {
  console.error(`Bundle size gate failed:\n- ${failures.join('\n- ')}`)
  process.exit(1)
}
console.log(`Bundle size gate passed (${files.length} JavaScript chunks)`)
