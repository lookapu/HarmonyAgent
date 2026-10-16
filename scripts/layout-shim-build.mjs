import { build } from 'vite'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 把 Tauri IPC shim 打成一个可注入的 IIFE 产物。
 *
 * 为什么需要打包：shim 要用官方 `mockIPC`（见 layout-fixtures.mjs 的说明），
 * 那是一个 ES module，没法直接当字符串塞进 `Page.addScriptToEvaluateOnNewDocument`。
 *
 * **每次测量都重新打包**：vite 启动约 0.3s，比起多一个「预构建产物会和 shim 源码脱节」
 * 的真源（那正是本项目反复踩的「只改一端」），这个成本可以忽略。
 */
const HERE = dirname(fileURLToPath(import.meta.url))
const REPO = resolve(HERE, '..')

export async function buildShim() {
  const outDir = mkdtempSync(join(tmpdir(), 'layout-shim-'))
  try {
    await build({
      root: REPO,
      logLevel: 'error',
      configFile: false,
      build: {
        outDir,
        emptyOutDir: true,
        lib: {
          entry: join(REPO, 'scripts', 'layout-probe-shim.js'),
          formats: ['iife'],
          name: 'LayoutProbeShim',
          fileName: () => 'shim.js',
        },
        minify: false, // 出错时要能直接读堆栈
      },
    })
    const code = readFileSync(join(outDir, 'shim.js'), 'utf-8')
    if (!code.includes('__probe')) {
      throw new Error('shim 产物里找不到 __probe——入口没有正确执行')
    }
    return code
  } finally {
    try { rmSync(outDir, { recursive: true, force: true }) } catch { /* ignore */ }
  }
}
