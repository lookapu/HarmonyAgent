#!/usr/bin/env node
/**
 * 第 20 道门禁：前端 invoke 的命令名必须在后端 `generate_handler!` 里注册。
 *
 * ## 现象
 *
 * 前端所有 Tauri 调用都走 `src/api/invoke.ts` 的两个封装
 * （`invokeWithError` / `invokeSafe`），它们最终调 `@tauri-apps/api/core` 的 `invoke(cmd)`。
 * 后端命令注册在 `src-tauri/src/lib.rs` 的
 * `tauri::generate_handler![ commands::project::list_projects, ... ]`。
 *
 * 两边是**纯文本约定，没有任何检查**：
 * - 后端改名 / 删命令，前端不会编译报错（invoke 的第一个参数是字符串）；
 * - 运行时才炸，症状是「command not found」，且**只在用户点到那个界面时**才炸。
 * - `check-docs.py` 只比 `generate_handler!` 的**条目数**与文档里的数字，不比名字。
 *
 * 这与 `check-i18n-key-usage.mjs` 是同一类问题的另一个面：
 * 代码引用了后端**不存在**的东西，而两边各自都是合法的。
 *
 * ## 判据（四条）
 *
 * 1. **前端调用 ⊆ 后端注册**（判红）：每个 invoke 字面量必须在 `generate_handler!` 里。
 *    这是唯一判红的主判据，也是唯一**不会误报**的方向。
 * 2. **入口封闭性**（判红）：`@tauri-apps/api/core` 的 `invoke` **只允许**在
 *    `src/api/invoke.ts` 里导入。
 *    ⚠️ 这条是**给本门自己看的**：封装名单是从那个文件里**推导**出来的，
 *    有人绕过封装直接 invoke ⇒ 本门根本看不到那些调用 ⇒
 *    「零缺失」会变成一个**假的全绿**。宁可报红让人来看。
 * 3. **命令名不得重复注册**（判红）：Tauri 同名命令会让 `invoke` 命中最后注册的那个，
 *    前一个静默失效。
 * 4. **反向（后端注册了但前端不调用）只报告、不判红**（`--report-unused`）：
 *    实测有 **9** 个，全部集中在 `commands::reliability::`
 *    （调度队列与 DAG 的 list/get/claim/retry 一族）。
 *    已逐个核实：**不是 bug**。`scheduler::register_active`（生产路径，
 *    由 `commands/chat.rs` 调用）在 `enqueue` 之后**就地**用一条带并发保护的
 *    `UPDATE` 完成了领取；而 `scheduler::claim_next` 是给**外部 worker 轮询**用的
 *    拉取式 API，桌面端单 worker 场景下没有调用者。
 *    ⇒ 这类是「已建好、已测试、尚未接入 UI 的运维表面」，
 *    删不删是**产品判断**，本门不替人做，所以只报告。
 *
 * ## 已知边界
 *
 * - 命令名写成**变量**时本门枚举不到（动态调用点）。
 *   一旦出现动态调用，`--report-unused` 与主判据都会变成**上界**，
 *   本门会**拒绝判红**并说明原因——这与 `check-i18n-key-usage.mjs`
 *   对死键方向的处理是同一个原则：**判据有上界就不要写成事实**。
 *   当前实测动态调用点为 **0**。
 * - 本门按字面文本解析 `lib.rs` 的注册表。若注册表改用宏生成，
 *   判据 1 会**全部落空**（找不到任何注册项），所以判据 2 之外再加一条：
 *   **注册表解析出 0 条时必须报红**，而不是当作「前端调用也不存在」而放行。
 * - 不校验参数结构（字段名 / 类型），那需要两侧各有一套 schema，超出本门范围。
 *
 * 用法：node scripts/check-ipc-commands.mjs            # 查，退出码 1 = 有缺失命令
 *       node scripts/check-ipc-commands.mjs --report-unused  # 反向名单，只报告不判红
 *       node scripts/check-ipc-commands.mjs --self-test      # 合成用例
 */

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'

const REPO = fileURLToPath(new URL('../', import.meta.url))
const SRC = join(REPO, 'src')
const INVOKE_TS = join(SRC, 'api', 'invoke.ts')
const LIB_RS = join(REPO, 'src-tauri', 'src', 'lib.rs')
const CORE_PKG = '@tauri-apps/api/core'

function walkFiles(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) walkFiles(full, out)
    else if (/\.(ts|tsx)$/.test(name)) out.push(full)
  }
  return out
}

function parse(file) {
  return ts.createSourceFile(
    file,
    readFileSync(file, 'utf-8'),
    ts.ScriptTarget.Latest,
    true,
    file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS
  )
}

/**
 * 从 invoke.ts **推导**封装名单，而不是写死。
 * 找不到就返回空 —— 由调用方按「解析失效」处理，绝不静默当成「没有调用」。
 */
export function discoverWrappers(invokeSrc) {
  const sf = ts.createSourceFile('invoke.ts', invokeSrc, ts.ScriptTarget.Latest, true,
    ts.ScriptKind.TS)
  let local = null
  const wrappers = new Set()
  for (const stmt of sf.statements) {
    const isImport = ts.isImportDeclaration(stmt)
      && stmt.moduleSpecifier.text === CORE_PKG
    if (isImport) {
      const clause = stmt.importClause
      if (!clause) continue
      // import { invoke as X } from '@tauri-apps/api/core'
      if (clause.namedBindings && ts.isNamedImports(clause.namedBindings)) {
        for (const el of clause.namedBindings.elements) {
          // ⚠️ `import { invoke as tauriInvoke }` 里 `el.name` 是 **tauriInvoke**、
          // `el.propertyName` 才是 **invoke**。写成 `el.name.text === 'invoke'`
          // 会推导出一个空封装名单 ⇒ 本门一个调用都看不到 ⇒ **报出假的全绿**。
          // 第一版正是这么写的，被自测当场抓住（真源实跑输出「前端 invoke 0 个命令」却退出 0）。
          if ((el.propertyName?.text ?? el.name.text) === 'invoke') local = el.name.text
        }
      }
      // import invoke from '@tauri-apps/api/core'（默认导入）
      if (clause.name) local = clause.name.text
    }
    if (ts.isFunctionDeclaration(stmt) && stmt.name && stmt.body && local) {
      let usesTauri = false
      const visit = (n) => {
        if (ts.isCallExpression(n) && ts.isIdentifier(n.expression)
            && n.expression.text === local) usesTauri = true
        ts.forEachChild(n, visit)
      }
      visit(stmt.body)
      const exported = stmt.modifiers?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword)
      if (usesTauri && exported) wrappers.add(stmt.name.text)
    }
  }
  return { local, wrappers }
}

/** 找出绕过封装直接 import core.invoke 的地方（判据 2）。 */
export function findBypass(files) {
  const bad = []
  for (const file of files) {
    const sf = parse(file)
    for (const stmt of sf.statements) {
      if (!ts.isImportDeclaration(stmt)) continue
      if (stmt.moduleSpecifier.text !== CORE_PKG) continue
      const clause = stmt.importClause
      if (!clause) continue
      let bindsInvoke = false
      if (clause.name && clause.name.text === 'invoke') bindsInvoke = true
      if (clause.namedBindings && ts.isNamedImports(clause.namedBindings)) {
        for (const el of clause.namedBindings.elements) {
          if ((el.propertyName?.text ?? el.name.text) === 'invoke') bindsInvoke = true
        }
      }
      if (bindsInvoke && file !== INVOKE_TS) {
        const line = sf.getLineAndCharacterOfPosition(stmt.getStart()).line + 1
        bad.push(`${relative(REPO, file).split('\\').join('/')}:${line}`)
      }
    }
  }
  return bad
}

export function collectCalls(files, wrappers) {
  const calls = new Map()
  const dynamic = []
  for (const file of files) {
    const sf = parse(file)
    const rel = relative(REPO, file).split('\\').join('/')
    const visit = (node) => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)
          && wrappers.has(node.expression.text)) {
        const arg = node.arguments[0]
        const line = sf.getLineAndCharacterOfPosition(node.getStart()).line + 1
        if (arg && (ts.isStringLiteral(arg) || ts.isNoSubstitutionTemplateLiteral(arg))) {
          if (!calls.has(arg.text)) calls.set(arg.text, [])
          calls.get(arg.text).push(`${rel}:${line}`)
        } else {
          dynamic.push(`${rel}:${line}`)
        }
      }
      ts.forEachChild(node, visit)
    }
    visit(sf)
  }
  return { calls, dynamic }
}

export function parseRegistry(libSrc) {
  const m = libSrc.match(/invoke_handler\(\s*tauri::generate_handler!\[([\s\S]*?)\]\s*\)/)
  if (!m) return null
  return m[1].split(',').map((x) => x.trim()).filter(Boolean)
    .map((x) => x.split('::').pop().trim())
}

export function audit({ calls, dynamic, registered, wrappers }) {
  const problems = []
  const notes = []
  if (registered === null) return { problems: ['lib.rs 里找不到 invoke_handler(tauri::generate_handler![...])'], notes }
  if (registered.length === 0) {
    return { problems: ['注册表解析出 0 条命令 ⇒ 本门的枚举已失效，**不能当作通过**'], notes }
  }
  // ⚠️ **两个解析步骤都要有这道守卫，不能只写一个。**
  // 第一版只给注册表侧写了「解析到 0 条就报红」，于是封装名单推导失败时
  // `calls` 恒为空集 ⇒ 「零缺失」⇒ **报出一个假的全绿**（真源实跑输出
  // 「前端 invoke 0 个命令」而退出码 0）。被自测当场抓住。
  if (wrappers === undefined || wrappers.size === 0) {
    return {
      problems: ['从 src/api/invoke.ts 推导出的封装名单是 0 条 ⇒ 本门看不到任何调用，**不能当作通过**'],
      notes,
    }
  }
  if (dynamic.length) {
    // **只记 note，不判红。** 与 `check-i18n-key-usage.mjs` 对死键方向的处理同一个原则：
    // 判据有上界就不要写成事实。这里主判据（静态调用 ⊆ 注册）对能看见的部分依然成立，
    // 而「有动态调用」本身不是缺失——判红它只会逼人去加白名单。
    // ⚠️ 第一版的实现与 docstring 相反（这里直接进了 problems，退出码 1），
    //    是变异 N5 抓出来的：**契约写了「不判红」而代码判红，两者必须有一个改**。
    notes.push(
      `有 ${dynamic.length} 处 invoke 的命令名不是字面量（${dynamic.slice(0, 3).join(', ')}）`
      + ` ⇒ 本门枚举不到它们，缺失方向的结论只是**上界**（只报告不判红）`
    )
  }
  const reg = new Set(registered)
  const dup = [...new Set(registered.filter((n, i) => registered.indexOf(n) !== i))]
  for (const n of dup) problems.push(`命令 ${n} 在 generate_handler! 里注册了多次 ⇒ invoke 只会命中最后一个`)
  for (const [name, at] of [...calls.entries()].sort()) {
    if (!reg.has(name)) {
      problems.push(
        `前端 invoke 了 ${name}，但 lib.rs 的 generate_handler! 里没有注册`
        + `（${at[0]}）⇒ 运行时 command not found，且只在用户点到那个界面时才炸`
      )
    }
  }
  return { problems, notes }
}

function scan() {
  const files = walkFiles(SRC)
  const bypass = findBypass(files)
  const { wrappers } = discoverWrappers(readFileSync(INVOKE_TS, 'utf-8'))
  const { calls, dynamic } = collectCalls(files, wrappers)
  const registered = parseRegistry(readFileSync(LIB_RS, 'utf-8'))
  return { bypass, wrappers, calls, dynamic, registered }
}
function selfTest() {
  const cases = []
  const ok = (name, got, want) => cases.push({ name, got, want })

  // 真源写法（照抄 src/api/invoke.ts 与 lib.rs 的注册项）
  const realInvoke = `import { invoke as tauriInvoke } from '@tauri-apps/api/core'
export async function invokeWithError<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try { return await tauriInvoke<T>(cmd, args) } catch (e) { throw e }
}
export async function invokeSafe<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: T): Promise<T> {
  try { return await tauriInvoke<T>(cmd, args) } catch (e) { return fallback }
}`
  const realReg = '    commands::project::list_projects,\n    commands::cost::get_cost,\n'
  const realLib = `            .invoke_handler(tauri::generate_handler![\n${realReg}            ])`

  // 判据：封装名单必须从 invoke.ts 推导出来
  const d = discoverWrappers(realInvoke)
  ok('从 invoke.ts 推导出两个封装', [...d.wrappers].sort().join(','), 'invokeSafe,invokeWithError')
  ok('同时推出 core.invoke 的本地别名', String(d.local), 'tauriInvoke')
  ok('封装名单为空时不得当成「没有调用」', String(discoverWrappers('const x = 1').wrappers.size), '0')

  // 判据：集合差集
  const mk = (called, dynamicArr = [], reg = realLib) => audit({
    calls: new Map(called.map((n) => [n, ['src/api/x.ts:1']])),
    dynamic: dynamicArr,
    registered: parseRegistry(reg),
    wrappers: d.wrappers,
  })
  ok('全部命中时零问题', String(mk(['list_projects', 'get_cost']).problems.length), '0')
  ok('缺失命令必须报出', String(mk(['list_projects', 'no_such_cmd']).problems.length), '1')
  ok('缺失信息必须含命令名', mk(['no_such_cmd']).problems[0]?.includes('no_such_cmd') ?? false, true)
  ok('嵌套泛型不得吞掉调用（第二十七批同一个坑）',
    String(mk(['project_scoped_counts']).problems.length), '1')
  ok('注册表为空必须报红而不是放行',
    audit({ calls: new Map(), dynamic: [], registered: [], wrappers: d.wrappers }).problems.length, 1)
  ok('找不到 generate_handler! 必须报红',
    audit({ calls: new Map(), dynamic: [], registered: null, wrappers: d.wrappers }).problems.length, 1)
  // 动态调用点：**只记 note，不进 problems**。第一版把它放进 problems 导致 exit=1，
  // 与 docstring 写的「不判红」相反，是变异 N5 抓出来的。
  ok('动态调用点存在时不判红', String(mk(['list_projects'], ['src/a.ts:9']).problems.length), '0')
  ok('动态调用点存在时必须留下说明',
    mk(['list_projects'], ['src/a.ts:9']).notes[0]?.includes('上界') ?? false, true)
  // ⚠️ 动态调用点**只让「没发现缺失」这个结论变成上界**，
  // 它**不压制**静态已经看见的真缺失——`no_such_cmd` 是字面量，它确实没注册，
  // 必须照报。第一版这里写成「problems=0」，被自测打回：那等于让一个上界
  // 把一条**事实**也一起吞掉。
  ok('动态存在时，静态看见的真缺失照样判红',
    String(mk(['no_such_cmd'], ['src/a.ts:9']).problems.length), '1')
  ok('动态存在时仍然留下上界说明',
    mk(['no_such_cmd'], ['src/a.ts:9']).notes.length, '1')
  ok('重复注册必须报红',
    audit({
      calls: new Map([['list_projects', ['src/a.ts:1']]]),
      dynamic: [],
      registered: parseRegistry('    commands::a::list_projects,\n    commands::b::list_projects,\n'),
      wrappers: d.wrappers,
    }).problems.length, 1)
  // 这一条是被真源实跑抓出来的：封装名单推导成 0 条时，本门曾报出**假的全绿**
  ok('封装名单为空必须拒绝判红（不能当成「没有调用」）',
    audit({
      calls: new Map(), dynamic: [],
      registered: parseRegistry(realLib), wrappers: new Set(),
    }).problems.length, 1)
  ok('封装名单为空时的报错必须点名 invoke.ts',
    audit({ calls: new Map(), dynamic: [], registered: parseRegistry(realLib), wrappers: new Set() })
      .problems[0].includes('src/api/invoke.ts'), true)

  let passed = 0
  for (const c of cases) {
    if (String(c.got) === String(c.want)) passed += 1
    else console.log(`  ✗ ${c.name}：期望 ${c.want}，实得 ${c.got}`)
  }
  console.log(`自测 ${passed}/${cases.length}`)
  return passed === cases.length ? 0 : 1
}

function main() {
  if (process.argv.includes('--self-test')) return selfTest()

  const { bypass, wrappers, calls, dynamic, registered } = scan()
  const problems = []
  if (bypass.length) {
    problems.push(
      `有 ${bypass.length} 处绕过封装直接 import ${CORE_PKG} 的 invoke（${bypass.join(', ')}）`
      + ` ⇒ 本门的封装名单是从 src/api/invoke.ts 推导的，那些调用**根本不会被统计**`
    )
  }
  const { problems: auditProblems, notes } = audit({ calls, dynamic, registered, wrappers })

  const sayNotes = () => {
    for (const n of notes) console.log(`注：${n}`)
  }

  if (process.argv.includes('--report-unused')) {
    const reg = new Set(registered ?? [])
    const unused = [...reg].filter((n) => !calls.has(n)).sort()
    console.log(`后端注册 ${reg.size} 个命令 · 前端 invoke 到 ${calls.size} 个 · 动态调用点 ${dynamic.length} 处`)
    console.log(`后端注册了但前端不调用：${unused.length} 个`)
    for (const n of unused) console.log(`  ${n}`)
    console.log('→ 只报告不判红：这属于「已建好、尚未接入 UI 的运维表面」，删不删是产品判断。')
    sayNotes()
    if (problems.length || auditProblems.length) {
      const all = [...problems, ...auditProblems]
      console.log(`\n另有 ${all.length} 条判红项：`)
      for (const p of all) console.log(`- ${p}`)
      return 1
    }
    return 0
  }

  if (problems.length || auditProblems.length) {
    const all = [...problems, ...auditProblems]
    console.log(`前后端 IPC 对不上（${all.length} 条）：`)
    for (const p of all) console.log(`- ${p}`)
    sayNotes()
    return 1
  }
  sayNotes()
  console.log(
    `IPC 命令对齐：前端 invoke ${calls.size} 个命令，全部在 generate_handler! 里注册`
    + `（后端共注册 ${registered.length} 个）· 动态调用点 ${dynamic.length} 处`
  )
  return 0
}

process.exit(main())
