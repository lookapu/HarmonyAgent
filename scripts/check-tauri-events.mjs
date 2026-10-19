#!/usr/bin/env node
/**
 * 第 21 道门禁：前端 listen 的事件名必须在后端 emit。
 *
 * ## 现象
 *
 * 第 20 道门禁管的是「前端 invoke 的命令名必须在后端 `generate_handler!` 里注册」。
 * Tauri 的**事件**是同一类问题的另一个面：
 *
 * - 前端用 `listen('evt', cb)`（`@tauri-apps/api/event`）订阅；
 * - 后端用 `app.emit("evt", payload)` 发射。
 *
 * 两边是**纯文本约定，没有任何检查**：
 * - 后端改名 / 删事件，前端**不会编译报错**（事件名是字符串字面量）；
 * - 运行时也不报错——`listen` 一个永远不来的事件，只是**永远不触发**；
 * - 症状是「某个功能就是不动」，且**没有任何地方报错**。
 *
 * 与 `check-i18n-key-usage.mjs` 同族：代码引用了后端**不存在**的东西，
 * 而两边各自都是合法的。
 *
 * ## 三方关系（本门要同时看住的三份名单）
 *
 * | 名单 | 位置 | 作用 |
 * |---|---|---|
 * | 后端发射 | `src-tauri/src` 下全部 `.rs` 的 `app.emit(...)` | 真源 |
 * | 桌面端订阅 | `src` 下全部 `.ts`/`.tsx` 的 `listen(...)` | Tauri 事件直收 |
 * | LAN 桥白名单 | `services/lan_server.rs` 的 `EVENT_WHITELIST` | 转成 SSE 转发给网页端 |
 *
 * 第三份是**后端自己维护的事件名名单**，与 `generate_handler!` 同类。
 * 名单里躺着一个没人发的事件 = 名册造假（死转发）。
 *
 * ⚠️ **命名空间不可混**：LAN 网页端还订阅了**服务端直发的 SSE 事件**
 * （`session-expired`，走 `tx.send("event: …")`，不经过 Tauri 事件），
 * 它不在白名单里是**正确的**。本门把「服务端直发的 SSE 事件」单列一桶，
 * 避免把它误报成「转发不到」。
 *
 * ## 判据（五条）
 *
 * 1. **桌面端订阅 ⊆ 后端发射**（判红）：每个 `listen('X')` 的 X 必须被后端 emit。
 *    订阅一个永远不来的事件，功能就是不动。
 * 2. **LAN 白名单 ⊆ 后端发射**（判红）：死转发项同样是名册造假。
 * 3. **解析失效守卫**（判红）：后端发射集合为空、或桌面端一个 `listen` 绑定都没解析到
 *    ⇒ 本门枚举失效，**不能当作通过**。
 *    ⚠️ **两个解析步骤都要有这道守卫**：第三十批踩过「只给一侧写」，
 *    结果封装名单推导失败 ⇒ 收集到 0 个调用 ⇒ 报出**假的全绿**。
 * 4. **枚举不完整时，判据 1/2 降级为「只报告」**：
 *    只要还有一个发射点的**事件名定不出来**，「没人发 X」就只是**上界**，
 *    不是事实——**看不见的事实不能报成事实**。
 *    ⚠️ 注意这里的极性与 IPC 门禁**相反**：动态的是**发射点**，
 *    它让「没人发」变成上界；而 IPC 门禁里动态的是**调用点**，
 *    它只让「没人调」变成上界，**不压制**静态看见的真缺失。
 *    两种极性都要说清楚，否则「上界不许吞掉事实」会被用反。
 * 5. **反向（后端发了但桌面端不订阅 / LAN 网页端订了但转发不到）只报告**，不判红。
 *
 * ## 事件名解析：三层，缺一层就产生假阳性
 *
 * 裸字面量正则只能看到 **90/91** 个 `.emit(` 调用点。剩下一个藏得很深：
 *
 * ```rust
 * // services/runtime_progress.rs —— 事件名是**函数参数**
 * pub fn emit(app: &tauri::AppHandle, event: &str, p: &RuntimeProgress) {
 *     let _ = app.emit(event, p);
 * }
 *
 * // services/node_runtime.rs —— 事件名是**调用方**的常量
 * const EVENT: &str = "node-runtime-progress";
 * runtime_progress::emit(app, EVENT, &RuntimeProgress::phase("check", "…"));
 * ```
 *
 * 而且它还**透传**一层：`runtime_progress::download_to_file(app, EVENT, …)`
 * 的体内把同一个参数交给同文件的 `emit(app, event, …)`。
 *
 * 只认字面量 ⇒ `node-runtime-progress` / `git-runtime-progress` /
 * `jdk-install-progress` 三个**真实存在**的事件会被判成「前端监听了没人发的事件」，
 * 也就是 **3 个假阳性**。假阳性比假阴性更贵：它会让人去删一个活着的监听。
 *
 * ⇒ 本门解析三层：**① 字面量 ② 同文件 `const X: &str = "…"` ③ 跨文件辅助函数的参数**
 * （含参数透传）。「事件源」用**不动点**求出：函数把��己的参数传进另一个事件源的
 * 事件槽时，它自己也是事件源。
 *
 * ## 已知边界
 *
 * - 只看**事件名**，不看 **payload 结构**。前端 `listen<{ a: string }>('x')` 与
 *   后端 `emit("x", OtherStruct { … })` 的字段是否对得上，本门不判。
 * - Rust 侧按**字面文本**解析。若将来改用宏批量生成 `emit` 调用，
 *   判据 1/2 会**全部落空**——所以判据 3 必须守住「发射集合为空就报红」。
 * - `listen` 的**首参不是字面量**（动态事件名）时本门枚举不到，同样降级为只报告。
 *
 * 用法：node scripts/check-tauri-events.mjs                  # 查，退出码 1 = 有对不上的
 *       node scripts/check-tauri-events.mjs --report-unused  # 反向名单，只报告不判红
 *       node scripts/check-tauri-events.mjs --self-test       # 合成用例
 */

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'

const REPO = fileURLToPath(new URL('../', import.meta.url))
const SRC = join(REPO, 'src')
const RS_SRC = join(REPO, 'src-tauri', 'src')
const LAN_SERVER = join(RS_SRC, 'services', 'lan_server.rs')
const LAN_UI = join(RS_SRC, 'services', 'lan_ui', 'app.js')
const EVENT_PKG = '@tauri-apps/api/event'

/** 工作区里的备份文件不进 git（`.gitignore` 有 `*.bak` / `*.before-rewrite`），
 *  但仍躺在磁盘上——按目录扫描会把它们一起扫进来。 */
const BACKUP_RE = /\.(bak|before-rewrite)$/

function walkFiles(dir, out = [], re = /\.(ts|tsx)$/) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) walkFiles(full, out, re)
    else if (re.test(name) && !BACKUP_RE.test(name)) out.push(full)
  }
  return out
}

function parseTs(file) {
  return ts.createSourceFile(
    file,
    readFileSync(file, 'utf-8'),
    ts.ScriptTarget.Latest,
    true,
    file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS
  )
}

// ===========================================================================
// 前端：收集 listen 订阅
// ===========================================================================

/**
 * 从 `@tauri-apps/api/event` 的导入里**推导**监听函数名。
 *
 * ⚠️ 不能写死 `'listen'`：`import { listen as onEvent } from …` 也是合法写法，
 *    写死会漏掉那一类；反过来若推导失败，**必须**让调用方看到「0 条」，
 *    不能当成「没有订阅」。
 */
export function discoverListenerNames(files) {
  const perFile = new Map()
  for (const file of files) {
    const sf = parseTs(file)
    const locals = new Set()
    for (const stmt of sf.statements) {
      if (!ts.isImportDeclaration(stmt)) continue
      if (stmt.moduleSpecifier.text !== EVENT_PKG) continue
      const clause = stmt.importClause
      if (!clause) continue
      if (clause.name) locals.add(clause.name.text)
      if (clause.namedBindings && ts.isNamedImports(clause.namedBindings)) {
        for (const el of clause.namedBindings.elements) {
          // ⚠️ `import { listen as onEvent }` 里 `el.name` 是 **onEvent**、
          // `el.propertyName` 才是 **listen**。判断反了会推出空名单
          // ⇒ 一个订阅都收不到 ⇒ **报出假的全绿**（第三十批同一个坑）。
          if ((el.propertyName?.text ?? el.name.text) === 'listen') locals.add(el.name.text)
        }
      }
    }
    if (locals.size) perFile.set(file, locals)
  }
  return perFile
}

export function collectSubscriptions(files, perFile) {
  const names = new Map()
  const dynamic = []
  for (const [file, locals] of perFile) {
    const sf = parseTs(file)
    const rel = relative(REPO, file).split('\\').join('/')
    const visit = (node) => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)
          && locals.has(node.expression.text)) {
        const arg = node.arguments[0]
        const line = sf.getLineAndCharacterOfPosition(node.getStart()).line + 1
        if (arg && (ts.isStringLiteral(arg) || ts.isNoSubstitutionTemplateLiteral(arg))) {
          if (!names.has(arg.text)) names.set(arg.text, [])
          names.get(arg.text).push(`${rel}:${line}`)
        } else {
          dynamic.push(`${rel}:${line}`)
        }
      }
      ts.forEachChild(node, visit)
    }
    visit(sf)
  }
  return { names, dynamic }
}

// ===========================================================================
// Rust：解析后端发射的事件名
// ===========================================================================

/** 把注释内容替换成等长空格：位置不变，但注释里的 `emit(` 不会被当成调用点。 */
export function maskComments(src) {
  const out = src.split('')
  let i = 0
  let inStr = false
  let esc = false
  while (i < src.length) {
    const ch = src[i]
    if (inStr) {
      if (esc) esc = false
      else if (ch === '\\') esc = true
      else if (ch === '"') inStr = false
      i += 1
      continue
    }
    if (ch === '"') { inStr = true; i += 1; continue }
    if (ch === '/' && src[i + 1] === '/') {
      while (i < src.length && src[i] !== '\n') { out[i] = ' '; i += 1 }
      continue
    }
    if (ch === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2)
      const stop = end < 0 ? src.length : end + 2
      for (let k = i; k < stop; k += 1) if (out[k] !== '\n') out[k] = ' '
      i = stop
      continue
    }
    i += 1
  }
  return out.join('')
}

/** 同文件里的 `const X: &str = "…"` / `static X: &str = "…"`。 */
export function parseConsts(src) {
  const consts = new Map()
  const re = /(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*&(?:'static\s+)?str\s*=\s*"((?:[^"\\]|\\.)*)"/g
  let m
  while ((m = re.exec(src))) consts.set(m[1], m[2])
  return consts
}

/** 从 `openIdx`（`(` 的位置）起，按**顶层**逗号切分参数；返回每个参数的**区间**。 */
export function splitArgRanges(src, openIdx) {
  const ranges = []
  let depth = 0
  let start = openIdx + 1
  let inStr = false
  let esc = false
  for (let i = openIdx + 1; i < src.length; i += 1) {
    const ch = src[i]
    if (inStr) {
      if (esc) esc = false
      else if (ch === '\\') esc = true
      else if (ch === '"') inStr = false
      continue
    }
    if (ch === '"') { inStr = true; continue }
    if (ch === '(' || ch === '[' || ch === '{') { depth += 1; continue }
    if (ch === ')' || ch === ']' || ch === '}') {
      if (depth === 0) { ranges.push([start, i]); return { ranges, end: i } }
      depth -= 1
      continue
    }
    if (ch === ',' && depth === 0) { ranges.push([start, i]); start = i + 1 }
  }
  ranges.push([start, src.length])
  return { ranges, end: src.length }
}

/** 括号/花括号的配对位置。 */
function matchPair(src, openIdx, open, close) {
  let depth = 0
  let inStr = false
  let esc = false
  for (let i = openIdx; i < src.length; i += 1) {
    const ch = src[i]
    if (inStr) {
      if (esc) esc = false
      else if (ch === '\\') esc = true
      else if (ch === '"') inStr = false
      continue
    }
    if (ch === '"') { inStr = true; continue }
    if (ch === open) depth += 1
    else if (ch === close) { depth -= 1; if (depth === 0) return i }
  }
  return -1
}

/** `fn` 声明：名字、参数名表、函数体区间。 */
const FN_RE = /(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(/g

export function parseFns(src) {
  const fns = []
  FN_RE.lastIndex = 0
  let m
  while ((m = FN_RE.exec(src))) {
    const openIdx = m.index + m[0].length - 1
    const closeIdx = matchPair(src, openIdx, '(', ')')
    if (closeIdx < 0) continue
    const { ranges } = splitArgRanges(src, openIdx)
    const paramNames = ranges.map(([a, b]) => {
      const head = src.slice(a, b).split(':')[0].trim()
      const cleaned = head.replace(/^(?:mut\s+|ref\s+)+/, '').trim()
      return /^[A-Za-z_][A-Za-z0-9_]*$/.test(cleaned) ? cleaned : null
    })
    const braceIdx = src.indexOf('{', closeIdx)
    let bodyStart = -1
    let bodyEnd = -1
    if (braceIdx >= 0) {
      bodyStart = braceIdx
      bodyEnd = matchPair(src, braceIdx, '{', '}')
    }
    fns.push({ name: m[1], index: m.index, openIdx, paramNames, bodyStart, bodyEnd })
  }
  return fns
}

/** 找到**包含** `pos` 的 fn（按函数体区间判定，不靠「前面最近的那个」）。 */
export function enclosingFn(fns, pos) {
  let best = null
  for (const f of fns) {
    if (f.bodyStart < 0) continue
    if (f.bodyStart <= pos && (f.bodyEnd < 0 || pos <= f.bodyEnd)) {
      if (!best || f.index > best.index) best = f
    }
  }
  return best
}

function lineOf(src, pos) {
  return src.slice(0, pos).split('\n').length
}

/** `.emit(` 调用点。**精确**方法名——`emit_log` / `emit_log_batch` 不是 Tauri 事件：
 *  它们的第一个字符串参数是日志流名（stdout/stderr/system），不是事件名。
 *  写成 `\.emit[a-z_]*\(` 会把 30 处 `emit_log("system", …)` 当成事件。 */
export function findEmitSites(src) {
  const masked = maskComments(src)
  const sites = []
  const re = /\.emit\(\s*("(?:[^"\\]|\\.)*"|[A-Za-z_][A-Za-z0-9_]*)/g
  let m
  while ((m = re.exec(masked))) {
    sites.push({ token: m[1], index: m.index, at: lineOf(src, m.index) })
  }
  return sites
}

/** 模块名（文件主干），用于限定调用 `module::fn(` 的匹配。 */
function moduleOf(file) {
  return file.split(/[\\/]/).pop().replace(/\.rs$/, '')
}

/**
 * 找 `module::fn(` 与同文件 `fn(` 的调用点。
 *
 * ⚠️ **必须传「掩掉注释」的源码**：注释里出现的 `emit(` / `download_to_file(`
 * 会被当成真调用点，而它的实参解析不出事件名 ⇒ 门禁**误降级为只报告**
 * ⇒ 本该判红的东西悄悄溜过去。掩码保持**长度不变**，所以位置与行号都对得上，
 * 切出来的区间仍可拿去原文取字。
 */
export function findCallSites(src, start, end, moduleName, fnName) {
  const out = []
  const patterns = [
    { re: new RegExp(`\\b${moduleName}::${fnName}\\s*\\(`, 'g'), qualified: true },
    { re: new RegExp(`(^|[^.\\w:])${fnName}\\s*\\(`, 'g'), qualified: false },
  ]
  for (const { re, qualified } of patterns) {
    re.lastIndex = Math.max(0, start - 1)
    let m
    while ((m = re.exec(src))) {
      if (m.index >= end) break
      const openIdx = src.indexOf('(', m.index + m[0].length - 1)
      if (openIdx < 0) continue
      // 跳过**函数定义本身**（`fn emit(` 不是调用点）
      const before = src.slice(Math.max(0, m.index - 6), m.index)
      if (/\bfn\s*$/.test(before)) continue
      const { ranges } = splitArgRanges(src, openIdx)
      out.push({ index: m.index, openIdx, ranges, qualified, at: lineOf(src, m.index) })
    }
  }
  return out
}

/**
 * 求出所有「事件源」：事件名从它的参数流出去的函数。
 *
 * - 直接：函数体里有 `.emit(<本函数的参数>, …)`；
 * - 间接：函数体把**自己的某个参数**传进了另一个事件源的事件槽。
 *   用**不动点**求：先取直接源，再一轮轮补间接源，直到没有新增。
 */
export function collectSources(modules) {
  const sources = []
  const key = (s) => `${s.file}::${s.fnName}`
  for (const mod of modules) {
    for (const site of findEmitSites(mod.src)) {
      if (site.token.startsWith('"')) continue
      const fn = enclosingFn(mod.fns, site.index)
      if (!fn) continue
      const slot = fn.paramNames.indexOf(site.token)
      if (slot < 0) continue
      sources.push({ file: mod.file, fnName: fn.name, slot, paramName: site.token })
    }
  }
  let changed = true
  while (changed) {
    changed = false
    for (const mod of modules) {
      for (const fn of mod.fns) {
        if (fn.bodyStart < 0) continue
        if (sources.some((s) => s.file === mod.file && s.fnName === fn.name)) continue
        for (const s of sources) {
          const module = moduleOf(s.file)
          const calls = findCallSites(mod.masked, fn.bodyStart, fn.bodyEnd, module, s.fnName)
          for (const c of calls) {
            // 限定调用要求模块对上；未限定调用只在**同文件**里算数
            if (c.qualified) {
              const qualifier = mod.src.slice(c.index, c.openIdx)
              if (!qualifier.includes(`${module}::`)) continue
            } else if (s.file !== mod.file) continue
            const arg = (mod.src.slice(c.ranges[s.slot][0], c.ranges[s.slot][1]) ?? '').trim()
            const ident = arg.replace(/^&/, '').trim()
            if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(ident)) continue
            const slot = fn.paramNames.indexOf(ident)
            if (slot < 0) continue
            sources.push({ file: mod.file, fnName: fn.name, slot, paramName: ident })
            changed = true
            break
          }
          if (changed) break
        }
      }
    }
  }
  return sources
}

/**
 * 把所有 `.emit(` 调用点分类：**已定值** / **不可判定**。
 *
 * ⚠️ 这个分类必须**一处**做，不能分散在各处——第三十批踩过「只给一侧写零条守卫」，
 *    结果封装名单推导失败 ⇒ 收集到 0 个调用 ⇒ 报出**假的全绿**。
 *    本函数的第一版就漏了「直接 emit 点的标识符既不是常量也不是事件源参数」
 *    这一路：那种点被**静默丢弃**，于是「发射集合是下界」这件事根本没被报出来。
 *
 * - 字面量 → 已定值
 * - 同文件 `const X: &str = "…"` → 已定值
 * - 事件源的参数（本函数或经透传链）→ 由 `resolveSourceNames` 定值
 * - 其它（局部变量、无法解析的表达式）→ **不可判定**
 */
export function classifyEmitSites(modules, sources) {
  const emitted = new Map()
  const undetermined = []
  const add = (name, at) => {
    if (!emitted.has(name)) emitted.set(name, [])
    emitted.get(name).push(at)
  }
  for (const mod of modules) {
    for (const site of findEmitSites(mod.src)) {
      const at = `${mod.rel}:${site.at}`
      if (site.token.startsWith('"')) { add(site.token.slice(1, -1), at); continue }
      if (mod.consts.has(site.token)) { add(mod.consts.get(site.token), at); continue }
      const fn = enclosingFn(mod.fns, site.index)
      const covered = fn && sources.some((s) => s.file === mod.file && s.fnName === fn.name)
      if (covered) continue // 交给 resolveSourceNames 定值
      undetermined.push({
        at,
        why: `事件名「${site.token}」既不是字面量，也不是可解析的常量或事件源参数`,
      })
    }
  }
  return { emitted, undetermined }
}

/**
 * 把事件源解析成具体的事件名。
 *
 * 解析路径：调用点的第 `slot` 个实参 → 字面量 / 调用方文件的常量 /
 * 调用方的参数（⇒ 递归到调用方这个事件源）。
 * 解析不出来 ⇒ 归入 `undetermined`：发射集合从此只是**下界**。
 */
export function resolveSourceNames(modules, sources) {
  const names = new Map()
  const undetermined = []
  const byKey = new Map()
  for (const s of sources) byKey.set(`${s.file}::${s.fnName}`, s)

  const resolve = (source, visited) => {
    const module = moduleOf(source.file)
    for (const mod of modules) {
      const calls = findCallSites(mod.masked, 0, mod.masked.length, module, source.fnName)
      for (const c of calls) {
        if (c.qualified) {
          const qualifier = mod.src.slice(c.index, c.openIdx)
          if (!qualifier.includes(`${module}::`)) continue
        } else if (mod.file !== source.file) continue
        const range = c.ranges[source.slot]
        if (!range) {
          undetermined.push({ at: `${mod.rel}:${c.at}`, why: `调用点缺少第 ${source.slot + 1} 个实参` })
          continue
        }
        const arg = mod.src.slice(range[0], range[1]).trim()
        if (arg.startsWith('"')) {
          const name = arg.slice(1, -1)
          if (!names.has(name)) names.set(name, [])
          names.get(name).push(`${mod.rel}:${c.at}`)
          continue
        }
        const ident = arg.replace(/^&/, '').trim()
        if (mod.consts.has(ident)) {
          const name = mod.consts.get(ident)
          if (!names.has(name)) names.set(name, [])
          names.get(name).push(`${mod.rel}:${c.at}`)
          continue
        }
        // 实参是调用方函数的参数 ⇒ 事件名继续往上走
        const caller = enclosingFn(mod.fns, c.index)
        if (caller) {
          const callerSlot = caller.paramNames.indexOf(ident)
          const callerSource = byKey.get(`${mod.file}::${caller.name}`)
          if (callerSource && callerSlot >= 0) {
            const k = `${mod.file}::${caller.name}`
            if (visited.has(k)) {
              undetermined.push({ at: `${mod.rel}:${c.at}`, why: `事件名经 ${caller.name} 循环透传` })
              continue
            }
            visited.add(k)
            resolve(callerSource, visited)
            continue
          }
        }
        undetermined.push({
          at: `${mod.rel}:${c.at}`,
          why: `事件源 ${source.fnName} 的第 ${source.slot + 1} 个实参「${arg || '（缺失）'}」定不出事件名`,
        })
      }
    }
  }

  for (const s of sources) resolve(s, new Set([`${s.file}::${s.fnName}`]))
  return { names, undetermined }
}

// ===========================================================================
// LAN 桥
// ===========================================================================

/** `EVENT_WHITELIST` —— 后端自己维护的 Tauri 事件名名单。 */
export function parseWhitelist(src) {
  const block = src.match(/const\s+EVENT_WHITELIST[^=]*=\s*&\[([\s\S]*?)\]/)
  if (!block) return null
  const names = []
  const re = /"([^"]+)"/g
  let m
  while ((m = re.exec(block[1]))) names.push(m[1])
  return names
}

/** 服务端**直发**的 SSE 事件（不经过 Tauri 事件，也不走白名单）。 */
export function parseDirectSseEvents(src) {
  const names = new Set()
  const re = /event:\s*([A-Za-z0-9_:-]+)/g
  let m
  while ((m = re.exec(src))) names.add(m[1])
  return names
}

/**
 * 把 JS 注释内容替换成等长空格：位置不变，但注释里的 `on('…')` 不会被当成订阅。
 *
 * ⚠️ 必须**跟踪字符串**：LAN 网页端里有 `https://…` 这类字面量，
 * 朴素的「按 `//` 切到行尾」会把 URL 之后的内容整段吃掉。
 */
export function maskJsComments(src) {
  const out = src.split('')
  let i = 0
  let quote = null
  let esc = false
  while (i < src.length) {
    const ch = src[i]
    if (quote) {
      if (esc) esc = false
      else if (ch === '\\') esc = true
      else if (ch === quote) quote = null
      i += 1
      continue
    }
    if (ch === "'" || ch === '"' || ch === '`') { quote = ch; i += 1; continue }
    if (ch === '/' && src[i + 1] === '/') {
      while (i < src.length && src[i] !== '\n') { out[i] = ' '; i += 1 }
      continue
    }
    if (ch === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2)
      const stop = end < 0 ? src.length : end + 2
      for (let k = i; k < stop; k += 1) if (out[k] !== '\n') out[k] = ' '
      i = stop
      continue
    }
    i += 1
  }
  return out.join('')
}

/** LAN 网页端的 `on('X', …)` 订阅。 */
export function parseLanUiSubscriptions(src) {
  const names = new Map()
  const masked = maskJsComments(src)
  const re = /\bon\(\s*'([^']+)'/g
  let m
  while ((m = re.exec(masked))) {
    if (!names.has(m[1])) names.set(m[1], [])
    names.get(m[1]).push(src.slice(0, m.index).split('\n').length)
  }
  return names
}

// ===========================================================================
// 判据
// ===========================================================================

export function audit({ listened, listenDynamic, emitted, undetermined, whitelist }) {
  const problems = []
  const notes = []
  const emittedNames = new Set(emitted.keys())

  if (emittedNames.size === 0) {
    return { problems: ['后端发射集合解析出 0 个事件 ⇒ 本门的枚举已失效，**不能当作通过**'], notes }
  }
  if (!listened || listened.size === 0) {
    return { problems: ['桌面端一个 listen 订阅都没解析到 ⇒ 本门看不到任何订阅，**不能当作通过**'], notes }
  }
  if (listenDynamic.length) {
    notes.push(
      `有 ${listenDynamic.length} 处 listen 的事件名不是字面量（${listenDynamic.slice(0, 3).join(', ')}）`
      + ` ⇒ 本门枚举不到它们，缺失方向的结论只是**上界**（只报告不判红）`
    )
  }
  if (undetermined.length) {
    notes.push(
      `有 ${undetermined.length} 处后端发射点的事件名定不出来`
      + `（${undetermined.slice(0, 2).map((u) => `${u.at} ${u.why}`).join('；')}）`
      + ` ⇒ 发射集合只是**下界**，「没人发 X」是上界而非事实，本门降级为只报告`
    )
  }
  // 枚举不完整 ⇒ 「没人发」只是上界 ⇒ 判据 1/2 降级为只报告。
  // ⚠️ 极性说明：动态的是**发射点**，它让「没人发」变上界；
  //    这与 IPC 门禁里动态**调用点**只让「没人调」变上界是同一个原则，
  //    但落到**相反**的判据上——所以两边都要写清楚。
  const complete = undetermined.length === 0

  // 上界方向上的正确输出是「已看见 N 个问题 + 另外 M 处我看不清」，
  // 而不是把疑似项**静默丢掉**——降级时它们进 notes，仍然可见。
  const report = (msg) => { if (complete) problems.push(msg); else notes.push(`疑似：${msg}`) }

  // 判据 1：桌面端订阅 ⊆ 后端发射
  for (const [name, at] of [...listened.entries()].sort()) {
    if (!emittedNames.has(name)) {
      report(
        `桌面端 listen 了事件 ${name}，但后端没有任何 emit 发射它（${at[0]}）`
        + ` ⇒ 该订阅**永远不会触发**，功能静默失效`
      )
    }
  }

  // 判据 2：LAN 白名单 ⊆ 后端发射
  if (whitelist) {
    for (const name of whitelist) {
      if (!emittedNames.has(name)) {
        report(
          `LAN 桥白名单 EVENT_WHITELIST 含事件 ${name}，但后端没有任何 emit 发射它`
          + ` ⇒ 这是**死转发项**（名册造假），转发链路里有一条永不触发的登记`
        )
      }
    }
  } else {
    notes.push('lan_server.rs 里找不到 EVENT_WHITELIST ⇒ 判据 2 未生效（不是通过）')
  }

  return { problems, notes, complete }
}

// ===========================================================================
// 扫描
// ===========================================================================

export function scan() {
  const tsFiles = walkFiles(SRC)
  const rsFiles = walkFiles(RS_SRC, [], /\.rs$/)

  const perFile = discoverListenerNames(tsFiles)
  const { names: listened, dynamic: listenDynamic } = collectSubscriptions(tsFiles, perFile)

  const modules = rsFiles.map((file) => {
    const src = readFileSync(file, 'utf-8')
    return {
      file,
      rel: relative(REPO, file).split('\\').join('/'),
      src,
      masked: maskComments(src),
      consts: parseConsts(src),
      fns: parseFns(src),
    }
  })

  const emitted = new Map()
  const add = (name, at) => {
    if (!emitted.has(name)) emitted.set(name, [])
    emitted.get(name).push(at)
  }
  // ③ 跨文件辅助函数（含参数透传）
  const sources = collectSources(modules)
  // ① 字面量 + ② 同文件常量 + 「定不出来」的直报点
  const classified = classifyEmitSites(modules, sources)
  for (const [name, at] of classified.emitted) for (const a of at) add(name, a)
  const resolved = resolveSourceNames(modules, sources)
  for (const [name, at] of resolved.names) for (const a of at) add(name, a)
  const undetermined = [...classified.undetermined, ...resolved.undetermined]

  let whitelist = null
  let directSse = new Set()
  let lanUi = new Map()
  if (exists(LAN_SERVER)) {
    const rs = readFileSync(LAN_SERVER, 'utf-8')
    whitelist = parseWhitelist(rs)
    directSse = parseDirectSseEvents(rs)
  }
  if (exists(LAN_UI)) lanUi = parseLanUiSubscriptions(readFileSync(LAN_UI, 'utf-8'))

  return {
    listened, listenDynamic, emitted, undetermined, whitelist, directSse, lanUi,
    importFiles: perFile.size, sources: sources.length,
  }
}

function exists(p) {
  try { statSync(p); return true } catch { return false }
}

// ===========================================================================
// 自测
// ===========================================================================

function selfTest() {
  const cases = []
  const ok = (name, got, want) => cases.push({ name, got, want })

  // --- 前端：订阅收集 ---
  const collectFromSource = (sources) => {
    const names = new Map()
    const dynamic = []
    for (let i = 0; i < sources.length; i += 1) {
      const sf = ts.createSourceFile(`f${i}.ts`, sources[i], ts.ScriptTarget.Latest, true, ts.ScriptKind.TS)
      const locals = new Set()
      for (const stmt of sf.statements) {
        if (!ts.isImportDeclaration(stmt)) continue
        if (stmt.moduleSpecifier.text !== EVENT_PKG) continue
        const clause = stmt.importClause
        if (!clause) continue
        if (clause.name) locals.add(clause.name.text)
        if (clause.namedBindings && ts.isNamedImports(clause.namedBindings)) {
          for (const el of clause.namedBindings.elements) {
            if ((el.propertyName?.text ?? el.name.text) === 'listen') locals.add(el.name.text)
          }
        }
      }
      const visit = (node) => {
        if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && locals.has(node.expression.text)) {
          const arg = node.arguments[0]
          if (arg && (ts.isStringLiteral(arg) || ts.isNoSubstitutionTemplateLiteral(arg))) {
            if (!names.has(arg.text)) names.set(arg.text, [])
            names.get(arg.text).push(`f${i}.ts:1`)
          } else dynamic.push(`f${i}.ts:1`)
        }
        ts.forEachChild(node, visit)
      }
      visit(sf)
    }
    return { names, dynamic }
  }

  const c1 = collectFromSource([
    `import { listen } from '@tauri-apps/api/event'
     listen('chat-stream', (e) => {})`,
    `import { listen as onEvent } from '@tauri-apps/api/event'
     onEvent<{ conversation_id: string; delta: string }>('chat-reasoning', (e) => {})`,
    `import { listen } from '@tauri-apps/api/event'
     listen('a', (e) => {})
     listen('b', (e) => {})`,
    `import { listen } from '@tauri-apps/api/event'
     listen(someVar, (e) => {})`,
  ])
  ok('收集到 4 个事件名', String(c1.names.size), '4')
  ok('泛型形态的 listen 不得被吞', String(c1.names.has('chat-reasoning')), 'true')
  ok('别名导入的 listen 不得被吞', String(c1.names.has('chat-reasoning')), 'true')
  ok('动态事件名进 dynamic', String(c1.dynamic.length), '1')
  ok('动态事件名不得进 names', String(c1.names.has('someVar')), 'false')
  ok('DOM 事件不得被当成 Tauri 事件',
    String(collectFromSource([`window.addEventListener('deveco:open-file', () => {})`]).names.size), '0')

  // --- Rust：字面量 / 常量 / emit_log 排除 ---
  const mkMod = (file, src) => ({
    file, rel: file, src, masked: maskComments(src),
    consts: parseConsts(src), fns: parseFns(src),
  })
  const r1 = mkMod('/fake/a.rs', `fn go(app: &AppHandle) {
  let _ = app.emit("chat-stream", payload);
}`)
  ok('字面量 emit 被解析', String(findEmitSites(r1.src).length), '1')
  ok('字面量事件名正确',
    String(findEmitSites(r1.src)[0].token.slice(1, -1)), 'chat-stream')
  const r2 = mkMod('/fake/b.rs', `fn go(app: &AppHandle) {
  const EVENT: &str = "node-runtime-progress";
  let _ = app.emit(EVENT, payload);
}`)
  ok('同文件常量可被解析', String(r2.consts.get('EVENT')), 'node-runtime-progress')
  const r3 = mkMod('/fake/c.rs', `fn go(ctx: &Ctx) {
  ctx.emit_log("system", "命令已由用户中断");
  ctx.emit_log_batch("stdout", &lines);
}`)
  ok('emit_log 不得被当成 emit（30 处假事件）', String(findEmitSites(r3.src).length), '0')

  // --- 注释里的 emit( 不得被当成调用点 ---
  const r4 = mkMod('/fake/d.rs', `/// 思考增量（透传 emit chat-reasoning）
fn go(app: &AppHandle) {
  let _ = app.emit("chat-stream", payload);
}`)
  ok('注释里的 emit( 不得被当成调用点', String(findEmitSites(r4.src).length), '1')

  // --- 直接 emit 点的标识符定不出来 ⇒ 必须进 undetermined（不得静默丢弃） ---
  // 这一条是变异 N6 抓出来的：那个点原先被**静默丢掉**，
  // 于是「发射集合只是下界」这件事根本没被报出来，判据 1 仍在按完整集合判红。
  const dynMod = mkMod('/fake/e.rs', `fn go(app: &AppHandle) {
  let name = String::from("x");
  let _ = app.emit(name, payload);
}`)
  const cls = classifyEmitSites([dynMod], [])
  ok('定不出的直接 emit 点必须进 undetermined', String(cls.undetermined.length), '1')
  ok('定不出的直接 emit 点不得进已定值集合', String(cls.emitted.size), '0')
  const cls2 = classifyEmitSites(
    [mkMod('/fake/f.rs', `fn go(app: &AppHandle) {\n  let _ = app.emit(name, payload);\n}`)],
    [{ file: '/fake/f.rs', fnName: 'go', slot: 0, paramName: 'name' }],
  )
  ok('事件源覆盖的点不得误报 undetermined', String(cls2.undetermined.length), '0')
  ok('事件源覆盖的点也不得进已定值集合（交给 resolveSourceNames）', String(cls2.emitted.size), '0')

  // --- 注释里的调用点不得被当成真调用（否则门禁会误降级为只报告） ---
  const cmtMod = mkMod('/fake/g.rs', `// 旧写法：emit(app, EVENT, payload);
fn go(app: &AppHandle) {
  let _ = app.emit("chat-stream", payload);
}`)
  ok('注释里的调用点不得被算成调用点',
    String(findCallSites(cmtMod.masked, 0, cmtMod.masked.length, 'fake', 'emit').length), '0')

  // --- 事件源：参数 + 透传 ---
  const rpSrc = `pub fn emit(app: &tauri::AppHandle, event: &str, p: &RuntimeProgress) {
  let _ = app.emit(event, p);
}
pub async fn download_to_file(app: &tauri::AppHandle, event: &str, client: &reqwest::Client) -> Result<(), String> {
  emit(app, event, &RuntimeProgress::phase("download", "…"));
  Ok(())
}`
  const nodeSrc = `const EVENT: &str = "node-runtime-progress";
fn install(app: &AppHandle) {
  runtime_progress::emit(app, EVENT, &RuntimeProgress::phase("check", "…"));
  runtime_progress::download_to_file(app, EVENT, client);
}`
  const mods = [mkMod('/fake/runtime_progress.rs', rpSrc), mkMod('/fake/node_runtime.rs', nodeSrc)]
  const sources = collectSources(mods)
  ok('事件源至少 2 个（emit + 透传的 download_to_file）', String(sources.length >= 2), 'true')
  const resolved2 = resolveSourceNames(mods, sources)
  ok('跨文件常量解析出 node-runtime-progress',
    String(resolved2.names.has('node-runtime-progress')), 'true')
  ok('透传链不得留下未解析项', String(resolved2.undetermined.length), '0')

  // --- 参数列表切分 ---
  const sp = splitArgRanges('f(a, b, &RuntimeProgress::phase("x", "y"), d)', 1)
  ok('顶层逗号切分正确', String(sp.ranges.length), '4')
  ok('嵌套调用里的逗号不得切开',
    String('f(a, b, &RuntimeProgress::phase("x", "y"), d)'.slice(sp.ranges[2][0], sp.ranges[2][1]).includes('"x"')), 'true')

  // --- 常量解析 ---
  const pc = parseConsts('pub const EVENT: &str = "a-b";\nconst X: &str = "c";')
  ok('常量表解析出 2 条', String(pc.size), '2')
  ok('常量值正确', String(pc.get('EVENT')), 'a-b')

  // --- LAN 白名单 / 直发 SSE / 网页端订阅 ---
  const wl = parseWhitelist('const EVENT_WHITELIST: &[&str] = &[\n "chat-stream",\n "chat-done",\n];')
  ok('白名单解析出 2 条', String(wl.length), '2')
  ok('白名单找不到时必须返回 null', String(parseWhitelist('const X: u8 = 1;')), 'null')
  const sse = parseDirectSseEvents('tx.send("event: session-expired\\ndata: {}\\n\\n")')
  ok('服务端直发 SSE 事件被解析', String(sse.has('session-expired')), 'true')
  const lan = parseLanUiSubscriptions("on('chat-stream', () => {}); on('session-expired', () => {});")
  ok('LAN 网页端订阅解析出 2 个', String(lan.size), '2')

  // --- 判据 ---
  const mk = (listened, emitted, whitelist = null, undetermined = []) => audit({
    listened: new Map(listened.map((n) => [n, ['src/a.ts:1']])),
    listenDynamic: [],
    emitted: new Map(emitted.map((n) => [n, ['a.rs:1']])),
    undetermined,
    whitelist,
  })
  ok('全部命中时零问题', String(mk(['chat-stream'], ['chat-stream']).problems.length), '0')
  ok('死监听必须报出', String(mk(['chat-stream', 'chat-reasoning'], ['chat-stream']).problems.length), '1')
  ok('死监听信息必须含事件名',
    mk(['chat-reasoning'], ['chat-stream']).problems[0]?.includes('chat-reasoning') ?? false, true)
  ok('后端发射集合为空必须报红',
    audit({ listened: new Map([['a', ['x']]]), listenDynamic: [], emitted: new Map(), undetermined: [], whitelist: null }).problems.length, 1)
  ok('桌面端订阅集合为空必须报红（假全绿守卫）',
    audit({ listened: new Map(), listenDynamic: [], emitted: new Map([['a', ['x']]]), undetermined: [], whitelist: null }).problems.length, 1)
  ok('白名单死转发项必须报出',
    String(mk(['chat-stream'], ['chat-stream'], ['chat-stream', 'chat-reasoning']).problems.length), '1')
  ok('白名单里的正常事件不得误报',
    String(mk(['chat-stream'], ['chat-stream', 'chat-done'], ['chat-stream']).problems.length), '0')
  ok('白名单找不到时不得误报死转发',
    String(mk(['chat-stream'], ['chat-stream'], null).problems.length), '0')

  // 动态事件名：只记 note，不判红（判据有上界就不要写成事实）
  const dynNote = audit({
    listened: new Map([['chat-stream', ['x']]]), listenDynamic: ['src/a.ts:9'],
    emitted: new Map([['chat-stream', ['y']]]), undetermined: [], whitelist: null,
  })
  ok('动态订阅存在时不得判红', String(dynNote.problems.length), '0')
  ok('动态订阅存在时必须留下上界说明',
    String(dynNote.notes.some((n) => n.includes('上界'))), 'true')

  // ⚠️ **上界不许吞掉事实**：动态存在时，静态看见的真缺失照样判红
  ok('动态存在时静态真缺失照样判红',
    String(mk(['chat-reasoning'], ['chat-stream']).problems.length), '1')

  // 不可判定：发射集合只是下界 ⇒ 「没人发」是上界而非事实 ⇒ 不判红
  const und = mk(['node-runtime-progress'], ['chat-stream'], null,
    [{ at: 'a.rs:1', why: '辅助函数参数解析不出' }])
  ok('发射集合不完整时死监听不得判红（只报告）', String(und.problems.length), '0')
  ok('发射集合不完整时必须留下降级说明',
    String(und.notes.some((n) => n.includes('下界'))), 'true')
  ok('发射集合完整时同一组数据必须判红',
    String(mk(['node-runtime-progress'], ['chat-stream']).problems.length), '1')

  let passed = 0
  for (const c of cases) {
    if (String(c.got) === String(c.want)) passed += 1
    else console.log(`  ✗ ${c.name}：期望 ${c.want}，实得 ${c.got}`)
  }
  console.log(`自测 ${passed}/${cases.length}`)
  return passed === cases.length ? 0 : 1
}

// ===========================================================================
// 入口
// ===========================================================================

function main() {
  if (process.argv.includes('--self-test')) return selfTest()

  const s = scan()
  const { problems, notes } = audit(s)

  const sayNotes = () => { for (const n of notes) console.log(`注：${n}`) }

  if (process.argv.includes('--report-unused')) {
    const listened = new Set(s.listened.keys())
    const unused = [...s.emitted.keys()].filter((n) => !listened.has(n)).sort()
    console.log(`后端发射 ${s.emitted.size} 个事件 · 桌面端订阅 ${s.listened.size} 个 · 动态订阅点 ${s.listenDynamic.length} 处`)
    console.log(`后端发射了但桌面端不订阅：${unused.length} 个`)
    for (const n of unused) console.log(`  ${n}   @ ${s.emitted.get(n)[0]}`)
    // LAN 网页端订了但转发不到（白名单 ∪ 服务端直发 SSE 事件）
    const reach = new Set([...(s.whitelist ?? []), ...s.directSse])
    const lanUnreachable = [...s.lanUi.keys()].filter((n) => !reach.has(n)).sort()
    console.log(`LAN 网页端订阅 ${s.lanUi.size} 个 · 转发可达 ${reach.size} 个 · 订了但转发不到 ${lanUnreachable.length} 个`)
    for (const n of lanUnreachable) console.log(`  ${n}   @ lan_ui/app.js:${s.lanUi.get(n).join(',')}`)
    console.log('→ 只报告不判红：后端发了但桌面端不订阅，属于「已建好、尚未接入 UI 的运维表面」。')
    sayNotes()
    if (problems.length) {
      console.log(`\n另有 ${problems.length} 条判红项：`)
      for (const p of problems) console.log(`- ${p}`)
      return 1
    }
    return 0
  }

  if (problems.length) {
    console.log(`前后端 Tauri 事件对不上（${problems.length} 条）：`)
    for (const p of problems) console.log(`- ${p}`)
    sayNotes()
    return 1
  }
  sayNotes()
  console.log(
    `Tauri 事件对齐：桌面端订阅 ${s.listened.size} 个事件，全部有后端 emit`
    + `（后端共发射 ${s.emitted.size} 个）· LAN 白名单 ${(s.whitelist ?? []).length} 条 · 动态订阅点 ${s.listenDynamic.length} 处`
  )
  return 0
}

process.exit(main())
