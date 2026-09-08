import { lstat, readdir, rm } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const projectRoot = fileURLToPath(new URL('../', import.meta.url))
const dryRun = process.argv.includes('--dry-run')

// 只允许清理这里列出的可再生目录。node_modules、源码、配置和 .git 永不进入目标集。
const generatedDirectories = [
  'dist',
  'coverage',
  'src-tauri/target',
  'node_modules/.vite',
]

async function exists(target) {
  try {
    await lstat(target)
    return true
  } catch (error) {
    if (error?.code === 'ENOENT') return false
    throw error
  }
}

async function removeGeneratedDirectory(relativePath) {
  const target = path.resolve(projectRoot, relativePath)
  const relative = path.relative(projectRoot, target)
  if (!relative || relative.startsWith('..') || path.isAbsolute(relative)) {
    throw new Error(`拒绝清理项目外路径：${target}`)
  }
  if (!(await exists(target))) {
    console.log(`跳过（不存在）：${relativePath}`)
    return
  }
  if (dryRun) {
    console.log(`将清理：${relativePath}`)
    return
  }
  await rm(target, { recursive: true, force: true })
  console.log(`已清理：${relativePath}`)
}

async function removeFinderMetadata(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.name === '.git' || entry.name === 'node_modules' || entry.name === 'target') continue
    const target = path.join(directory, entry.name)
    if (entry.name === '.DS_Store') {
      if (dryRun) console.log(`将清理：${path.relative(projectRoot, target)}`)
      else await rm(target, { force: true })
    } else if (entry.isDirectory() && !entry.isSymbolicLink()) {
      await removeFinderMetadata(target)
    }
  }
}

for (const relativePath of generatedDirectories) {
  await removeGeneratedDirectory(relativePath)
}
await removeFinderMetadata(projectRoot)

console.log(dryRun ? '检查完成：未删除任何文件。' : '生成物清理完成；源码、.git 和 node_modules 已保留。')
