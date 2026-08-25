import { readdir, stat } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const assetsDir = fileURLToPath(new URL('../dist/assets/', import.meta.url))
const budgets = [
  { label: 'Home', pattern: /^Home-.*\.js$/, maxBytes: 750_000 },
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
