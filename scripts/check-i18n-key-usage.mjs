#!/usr/bin/env node
/**
 * 第 18 道门禁：代码引用的 i18n 键必须在词库里**存在**。
 *
 * ## 现象
 *
 * `common.off` / `md.copied` 这类键**词库里根本没有**，而 i18next 的 fallback
 * 是把**键名本身**原样渲染出来——于是界面上直接写着 `common.off`。
 * 更隐蔽的一种：`t('chat.queueFailed', '排队消息发送失败')` 带了中文默认值，
 * **英文用户看到的是中文**。
 *
 * `check-i18n-wording.py` 管不了这一面：它比的是「中英两侧对不对得上」，
 * 两边都缺的时候配平检查完全看不出少了一个键。
 *
 * ## 为什么用 TypeScript 解析器，而不是正则
 *
 * 第一版是 Python + 手写词法分析器（状态机剥注释）。**实测它错得很厉害**：
 * `Home.tsx` 424 个真引用里**丢了 380 个**，`ConfigPage.tsx` **75 个全丢**。
 * 根因是模板串：`setActionError(`${t('config.clearError')}: ${e}`)` —
 * `` `${...} `` 里是**真代码**，而那个状态机把整段模板当字符串置空了。
 *
 * ⇒ 仓库里已经有 TypeScript（`tsc` 天天跑），**该用它而不是自己造一个更差的**。
 * 现测：Home.tsx / ConfigPage.tsx / Markdown.tsx 的引用一个不丢，
 * 且注释里的死键引用照常不算。
 *
 * ## 只判两个方向，死键方向不判
 *
 * | 方向 | 判红 | 理由 |
 * |---|---|---|
 * | 命名空间存在、**整键不存在** | 是 | 真缺陷，界面上显示键名 |
 * | **命名空间本身不存在** | 是 | 症状同上。第一版把它**直接过滤掉**，等于把「命名空间拼错」藏起来了；实测真源该类为 0，判红不产生噪声 |
 * | 死键（词库有、静态引用不到） | **否，只报告** | 13 个文件用动态键 ``t(`...`)``，静态枚举不到 ⇒ 报出来的是**上界不是事实** |
 *
 * ## 已知边界
 *
 * - 复数形式（`foo` / `foo_other`）由 `check-i18n-wording.py` 的 `PLURAL_SUFFIX` 管。
 * - 键名常量（`{ keyName: 'home.timeline' }`）只计入「已引用」，
 *   **不参与缺失键判定**——`{ title: 'about.tauri' }` 这类非 i18n 字段会撞上同一个形态，
 *   放进缺失方向就会变成噪声源。
 * - 只认 `t(...)` 与 `i18n.t(...)`；包装过的 `useT()` 一类本门看不到。
 * - **死键方向（`--report-dead`）是上界，不是事实。** 三种覆盖不到的形态：
 *   1. **裸前缀动态键**：`` t(`home.${name}`) ``（name 是变量）⇒ 一个 `home.`
 *      就罩住上千个键。实测 38 处动态键覆盖 **1101** 个键。
 *   2. **元组数组传键**：`[['includeMessages', 'repro.messages'], ...]`——
 *      键在数组里，经变量传给 `t()`。
 *   3. **非 i18n 数据文件**：`src/data/providerTemplates.ts` 里直接写
 *      `'provider.catApi'`，而该文件不 import react-i18next。
 *   这三类合计让当前名单里的 6 个键**全部是活的**（已逐个人工核实），
 *   所以死键方向**只报告、不判红**。
 *
 * ⚠️ `--report-dead` 除了名单，还打印**本次普查的全部口径**
 * （词库键 / `t()` 引用处数与命中键数 / keymap 引用键数 / 动态覆盖数）。
 * 台账 `docs/TECH_DEBT.md` 里的数字一律引用这份输出——
 * 上一版那张表有两个数字出自一次性普查脚本，**换台机器复现不出来**
 * （普查数原始条目、门禁数去重键，两种口径混在一起没人说得清那个数是什么）。
 * **门禁里出现的数字，必须是门禁自己能重新打出来的数字。**
 *
 * 用法：node scripts/check-i18n-key-usage.mjs            # 查，退出码 1 = 有缺失键
 *       node scripts/check-i18n-key-usage.mjs --report-dead  # 死键方向，只报告不判红
 *       node scripts/check-i18n-key-usage.mjs --self-test # 合成用例
 */
import { readFileSync, readdirSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'

const REPO = fileURLToPath(new URL('../', import.meta.url))
const SRC = join(REPO, 'src')
const WORDBOOK = ['zh.json', 'en.json']

/**
 * 命名空间形态：至少一个点，首段小写字母开头，**段内允许下划线**。
 *
 * ⚠️ 第一版写的是 `[A-Za-z0-9]+`，**不接受下划线** ⇒
 * `cost.errKind.rate_limited` 这类键整个对门禁不可见 ⇒ 拼错了也不会报红。
 * 这个洞是被一次死键普查撞出来的：`cost.errKind.rate_limited` 被普查判成死键，
 * 而 CostPage.tsx 明明写着 `t('cost.errKind.rate_limited')`。
 * **普查与门禁给出矛盾结论时，先怀疑门禁。**
 */
const NS_SHAPE = /^[a-z][A-Za-z0-9_]*(\.[A-Za-z0-9_]+)+$/
/**
 * 「看起来是文件名」的形态：`diagram.svg` / `image.png`。
 * 这些会作为**调用参数**出现（`` `<img src="${name}.png">` ``、`<object data=...>`），
 * 和包装函数传的 i18n 键长得一样。
 * 实测 callArg 桶共 7 条，其中 2 条是文件名、5 条是真键 ⇒ 按扩展名分开是干净的。
 */
const FILE_SHAPE = /\.(svg|png|jpe?g|gif|webp|avif|bmp|ico|json|css|js|mjs|cjs|ts|tsx|wasm|woff2?|ttf)$/i
const TEST_FILE = /\.test\.tsx?$/
const CODE_EXT = /\.(ts|tsx)$/

export function flatten(obj, prefix = '') {
  const out = {}
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k
    if (v && typeof v === 'object' && !Array.isArray(v)) Object.assign(out, flatten(v, key))
    else out[key] = v
  }
  return out
}

export function wordbook() {
  const data = WORDBOOK.map((n) =>
    JSON.parse(readFileSync(join(SRC, 'i18n', n), 'utf-8')),
  )
  return {
    keys: new Set(data.flatMap((d) => Object.keys(flatten(d)))),
    namespaces: new Set(data.flatMap((d) => Object.keys(d))),
  }
}

/**
 * 在一个源文件里找 i18n 引用。
 * @returns {{refs: {key:string,line:number}[], keys: string[], dynamic: boolean}}
 */
export function findRefs(src, fileName) {
  const sf = ts.createSourceFile(
    fileName, src, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX,
  )
  const refs = []
  const keys = []
  const dynamicPrefixes = []
  let dynamic = false
  let isI18n = false

  const lineOf = (pos) => sf.getLineAndCharacterOfPosition(pos).line + 1

  /**
   * 本文件里**自己声明**的函数/常量名。
   *
   * 判据：键只可能经**本地包装函数**传递（`run(fn, 'config.lanStopFailed')`），
   * 而 `track('project.trust')` 这种是**import 进来的埋点 API** ——
   * 两者形态一模一样，必须分开，否则埋点名会被当成缺失键报红。
   * 「本地声明」是可判定的；靠函数名黑名单则是猜。
   */
  const locals = new Set()
  const collectLocals = (n) => {
    if (ts.isFunctionDeclaration(n) && n.name) locals.add(n.name.text)
    if (ts.isVariableDeclaration(n) && ts.isIdentifier(n.name)) locals.add(n.name.text)
    ts.forEachChild(n, collectLocals)
  }
  collectLocals(sf)

  const walk = (node) => {
    // 这个文件里 `t` 是不是 i18n 的 t
    if (ts.isImportDeclaration(node) && ts.isStringLiteral(node.moduleSpecifier)) {
      if (node.moduleSpecifier.text === 'react-i18next') isI18n = true
    }
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
      if (node.expression.text === 'useTranslation') isI18n = true
    }
    if (ts.isCallExpression(node)) {
      const callee = node.expression
      const isT =
        (ts.isIdentifier(callee) && callee.text === 't') ||
        (ts.isPropertyAccessExpression(callee) &&
          ts.isIdentifier(callee.name) && callee.name.text === 't')
      if (isT) {
        const arg = node.arguments[0]
        /**
         * ⚠️ **必须下探参数子树**，不能只看「参数本身就是字符串字面量」。
         * 真源有两种写法会让 `ts.isStringLiteral(arg)` 直接为假：
         *   `t(loading ? 'teamShare.loading' : 'teamShare.working')`  ← 三元
         *   `t(idx === 0 ? 'a.b' : 'c.d')`                              ← 同上
         * 这两类是**真引用却完全不可见**，拼错了也不会报红。
         * 而判据本身对**文件里其它位置**的字符串一视同仁会把埋点名
         * （`project.trust`）、模块名（`highlight.js`）算进来 ⇒ 十几个假阳性。
         * ⇒ 范围收紧到「`t(...)` 的第一个参数子树内」，两者兼顾。
         */
        if (arg) {
          const seen = new Set()
          const collect = (n) => {
            if (ts.isStringLiteral(n) && NS_SHAPE.test(n.text) && !seen.has(n.text)) {
              seen.add(n.text)
              refs.push({ key: n.text, line: lineOf(n.getStart(sf)) })
            }
            ts.forEachChild(n, collect)
          }
          collect(arg)
        }
        if (
          arg &&
          (ts.isTemplateExpression(arg) || ts.isNoSubstitutionTemplateLiteral(arg))
        ) {
          dynamic = true
          // `` t(`home.${x}`) `` 的**头文字面量**就是它能覆盖的键前缀。
          // 死键方向靠它把「可能被动态引用」的键排除掉。
          const head = ts.isTemplateExpression(arg)
            ? arg.head.text
            : (ts.isNoSubstitutionTemplateLiteral(arg) ? arg.text : '')
          if (head) dynamicPrefixes.push(head)
        }
      }
    }
    /**
     * 键**经本文件定义的包装函数**传递：`run(() => stopLanServer(), 'config.lanStopFailed')`，
     * 而 `run` 内部是 `t(failKey)` ⇒ AST 静态追不进去。
     * ⇒ 「调用本地函数、且参数是命名空间形态字符串」也算一份引用。
     *
     * 两个必须分开的形态（实测都真实存在）：
     *   本地包装函数 → 真键；import 进来的埋点 API（`track`）→ 不是键。
     * 实测 callArg 桶共 7 条：5 条是真键、2 条是 `diagram.svg` / `image.png`（按扩展名排除）。
     */
    if (ts.isCallExpression(node)) {
      const callee = node.expression
      const isLocalCall = ts.isIdentifier(callee) && locals.has(callee.text)
      if (isLocalCall) {
        for (const arg of node.arguments) {
          if (
            ts.isStringLiteral(arg) &&
            NS_SHAPE.test(arg.text) &&
            !FILE_SHAPE.test(arg.text) &&
            !refs.some((r) => r.key === arg.text)
          ) {
            refs.push({ key: arg.text, line: lineOf(arg.getStart(sf)) })
          }
        }
      }
    }
    // 键名常量：{ keyName: 'home.timeline' }（只计入 keys，不进 refs）
    if (
      ts.isPropertyAssignment(node) &&
      ts.isIdentifier(node.name) &&
      /^[a-z][A-Za-z0-9_]*$/.test(node.name.text) &&
      ts.isStringLiteral(node.initializer) &&
      NS_SHAPE.test(node.initializer.text)
    ) {
      keys.push(node.initializer.text)
    }
    ts.forEachChild(node, walk)
  }
  walk(sf)

  const empty = { refs: [], keys: [], dynamic: false, dynamicPrefixes: [] }
  if (TEST_FILE.test(fileName)) return empty
  if (!isI18n) return empty
  return { refs, keys, dynamic, dynamicPrefixes }
}

/**
 * 把引用分成 (缺失键, 命名空间不存在)。两个分类都要报红。
 *
 * 提成独立函数是为了让它**可自测**——第一版这段逻辑内联在扫描里，
 * 于是「把未知命名空间静默丢弃」这种变异**打不动任何自测**。
 */
export function classify(refs, known, namespaces) {
  const missing = []
  const unknownNs = []
  for (const { key, line } of refs) {
    if (!namespaces.has(key.split('.')[0])) unknownNs.push({ key, line })
    else if (!known.has(key)) missing.push({ key, line })
  }
  return { missing, unknownNs }
}

function scan() {
  const { keys: known, namespaces } = wordbook()
  const missing = []
  const unknownNs = []
  const used = new Set()
  const dynamicPrefixes = []
  let dynamicFiles = 0
  /**
   * 引用量分开记：keymap 里的键与 `t()` 里的键是**两条不同的来源**，
   * 混成一个 `used.size` 之后台账就没法复核「静态引用到底是多少」了。
   * 台账里的每个数字都必须是 `--report-dead` 直接打出来的，而不是某个一次性脚本。
   */
  let refCount = 0
  const tKeys = new Set()
  const keymapKeys = new Set()

  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name)
      const st = readdirSync(dir, { withFileTypes: true }).find((d) => d.name === name)
      if (st.isDirectory()) {
        if (name === 'node_modules' || name === 'dist') continue
        walk(full)
      } else if (CODE_EXT.test(name)) {
        const r = findRefs(readFileSync(full, 'utf-8'), name)
        if (r.dynamic) dynamicFiles += 1
        dynamicPrefixes.push(...r.dynamicPrefixes)
        for (const k of r.keys) { used.add(k); keymapKeys.add(k) }
        for (const ref of r.refs) {
          used.add(ref.key)
          tKeys.add(ref.key)
          refCount += 1
        }
        const rel = relative(REPO, full).split('\\').join('/')
        const hit = classify(r.refs, known, namespaces)
        for (const h of hit.missing) missing.push({ ...h, file: rel })
        for (const h of hit.unknownNs) unknownNs.push({ ...h, file: rel })
      }
    }
  }
  walk(SRC)

  /**
   * 死键：词库里有、且**既无静态引用、也不被任何动态前缀覆盖**。
   *
   * 动态前缀能罩住多少，决定这份名单有多可信。本仓有一个
   * `` t(`home.${name}`) ``（name 是变量），光一个裸前缀 `home.` 就罩住上千个键
   * ⇒ **排掉它之后剩下来的才叫「可信死键」**。
   *
   * ⚠️ 因此死键方向**只报告、不判红**：动态键一旦改成裸前缀（`home.`），
   * 覆盖面就退化到「整个命名空间」，任何名单都会失真。
   */
  const coveredByDynamic = new Set()
  for (const p of dynamicPrefixes) {
    for (const k of known) if (k.startsWith(p)) coveredByDynamic.add(k)
  }
  const dead = [...known].filter((k) => !used.has(k) && !coveredByDynamic.has(k)).sort()

  return {
    missing,
    unknownNs,
    dynamicFiles,
    dead,
    dynamicPrefixes,
    coveredByDynamic,
    stats: { known: known.size, refCount, tKeys: tKeys.size, keymapKeys: keymapKeys.size, used: used.size },
  }
}

function selfTest() {
  const cases = []
  const expect = (name, got, want) => cases.push({ name, got, want })
  // 大部分用例要**先让这个文件被判定成 i18n 文件**，否则 refs 会被清空
  // ——第一版自测就是栽在这里：断言在报「undefined.line」而不是报判据错。
  const I18N_HEAD = "import { useTranslation } from 'react-i18next'\nconst { t } = useTranslation()\n"
  const S = (code, file = 'x.tsx') => findRefs(I18N_HEAD + code, file)

  // ---- 解析器必须做到的事（前四条都是**真踩过的坑**）----
  expect('普通调用', S("t('home.a')\n").refs.map((r) => r.key), ['home.a'])
  expect('行注释里的不算', S("// t('home.a')\n").refs.length, 0)
  expect('块注释里的不算', S("/* t('home.a') */\n").refs.length, 0)
  expect('字符串里的不算', S('const s = "t(\'home.a\')"\n').refs.length, 0)
  // 这一条是第一版 Python 状态机**实丢 380/424 个**的那一类
  expect('模板插值里的要算（第一版在这里整段丢光）',
    S("setError(`${t('home.a')}: ${e}`)\n").refs.map((r) => r.key), ['home.a'])
  expect('i18n.t(...) 也要算', S("i18n.t('home.a')\n").refs.map((r) => r.key), ['home.a'])
  expect('行号要准', S("\n\nt('home.a')\n").refs[0].line, 5)
  expect('无命名空间形态的键不算', S("t('all'); t('2.3.0')\n").refs.length, 0)
  expect('**带下划线的键段**要算（第一版的 `[A-Za-z0-9]` 会漏掉 cost.errKind.rate_limited）',
    S("t('cost.errKind.rate_limited')\n").refs.map((r) => r.key), ['cost.errKind.rate_limited'])
  expect('键经**本地包装函数**传递也要算（run(fn, "config.lanStopFailed")）',
    S("const run = (f: unknown, k: string) => f\nrun(() => 1, 'config.lanStopFailed')\n").refs.map((r) => r.key),
    ['config.lanStopFailed'])
  expect('import 进来的埋点 API 的同形字符串**不算**（track("project.trust")）',
    S("track('project.trust')\n").refs.length, 0)
  expect('**三元表达式**里的键要算（t(loading ? "a.x" : "a.y")）——第一版看不见',
    S("t(loading ? 'teamShare.loading' : 'teamShare.working')\n").refs.map((r) => r.key),
    ['teamShare.loading', 'teamShare.working'])
  expect('参数子树外的同形字符串**不算**（埋点名 project.trust 不是 i18n 键）',
    S("track('project.trust')\n").refs.length, 0)
  expect('调用参数里像**文件名**的不算（diagram.svg / image.png）',
    S("use(\u0027diagram.svg\u0027)\nuse(\u0027image.png\u0027)\n").refs.length, 0)
  expect('非 i18n 文件不参与', findRefs("t('home.a')\n", 'x.tsx').refs.length, 0)
  expect('测试文件排除', S("t('home.a')\n", 'x.test.tsx').refs.length, 0)
  expect('动态键被标出来且不进 refs',
    [S('t(`home.${x}`)\n').dynamic, S('t(`home.${x}`)\n').refs.length], [true, 0])
  expect('键名常量计入 keys', S("const m = { keyName: 'home.a' }\n").keys, ['home.a'])

  // ---- 两个分类都要能判出来，且都不能被静默丢掉 ----
  const known = new Set(['home.ok'])
  const ns = new Set(['home'])
  expect('分类：命名空间存在但键不存在 → 缺失',
    classify([{ key: 'home.nope', line: 3 }], known, ns).missing.length, 1)
  expect('分类：命名空间不存在 → 另一类，不许被过滤掉',
    classify([{ key: 'chatty.nope', line: 4 }], known, ns).unknownNs.length, 1)
  expect('分类：存在的键不报',
    classify([{ key: 'home.ok', line: 5 }], known, ns).missing.length +
      classify([{ key: 'home.ok', line: 5 }], known, ns).unknownNs.length, 0)

  let passed = 0
  for (const c of cases) {
    const g = JSON.stringify(c.got)
    const w = JSON.stringify(c.want)
    if (g === w) passed += 1
    else console.log(`  ✗ ${c.name}：期望 ${w}，实得 ${g}`)
  }
  // 输出格式必须与其余 17 道门一致（`自测 N/N`）：
  // `check-self-test-counts.py` 解析的是**输出文本**，
  // 改成英文就会报「读不出用例条数」——契约被破坏时该改门禁，不是放宽解析。
  console.log(`自测 ${passed}/${cases.length}`)
  process.exit(passed === cases.length ? 0 : 1)
}

function main() {
  if (process.argv.includes('--self-test')) return selfTest()

  const {
    missing, unknownNs, dynamicFiles, dead, dynamicPrefixes, coveredByDynamic, stats,
  } = scan()

  // 死键只报告、退出码不受影响（理由见 scan() 的注释）
  if (process.argv.includes('--report-dead')) {
    console.log(`词库键 ${stats.known}（zh+en 并集）`)
    console.log(`t() 静态引用 ${stats.refCount} 处，命中 ${stats.tKeys} 个不同键`)
    console.log(`keymap 引用 ${stats.keymapKeys} 个键`)
    console.log(`被引用到的不同键合计 ${stats.used} / ${stats.known}`)
    console.log(`动态键 ${dynamicPrefixes.length} 处，覆盖 ${coveredByDynamic.size} 个键`)
    console.log(`可信死键（排除静态/keymap/动态覆盖）${dead.length} 个：`)
    for (const k of dead) console.log(`  ${k}`)
    console.log('→ 死键方向**只报告不判红**：动态键一旦写成裸前缀（如 `home.`），覆盖面退化到整个命名空间。')
    return 0
  }

  if (!missing.length && !unknownNs.length) {
    const leaf = Object.keys(flatten(
      JSON.parse(readFileSync(join(SRC, 'i18n', WORDBOOK[0]), 'utf-8')),
    )).length
    console.log(
      `词库无缺失键、无失效命名空间引用（zh.json 扁平叶子键 ${leaf} 个）。` +
      `另有 ${dynamicFiles} 个文件使用动态键 t(\`...\`)，死键方向不在本门判定范围内` +
      `（静态枚举不到，报出来的是上界不是事实；要名单用 --report-dead）。`,
    )
    return 0
  }
  if (missing.length) {
    console.log(`发现 ${missing.length} 处引用了词库里不存在的键（界面上会直接显示键名本身）：`)
    for (const h of missing) console.log(`- ${h.file}:${h.line}  t('${h.key}')`)
    console.log('  → 修法：在 zh.json / en.json 同时补上这个键（中英必须一起补）')
    console.log('    已经带默认值的更危险：默认值是中文的话，英文用户会看到中文。')
  }
  if (unknownNs.length) {
    console.log(`另有 ${unknownNs.length} 处引用的**命名空间根本不存在**（症状同上，键名原样显示）：`)
    for (const h of unknownNs) console.log(`- ${h.file}:${h.line}  t('${h.key}')`)
    console.log(`  → 现有命名空间：${[...wordbook().namespaces].sort().join('、')}`)
  }
  return 1
}

process.exit(main())
