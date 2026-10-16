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
 *
 * 用法：node scripts/check-i18n-key-usage.mjs            # 查，退出码 1 = 有缺失键
 *       node scripts/check-i18n-key-usage.mjs --self-test # 合成用例
 */
import { readFileSync, readdirSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'

const REPO = fileURLToPath(new URL('../', import.meta.url))
const SRC = join(REPO, 'src')
const WORDBOOK = ['zh.json', 'en.json']

/** 命名空间形态：至少一个点，首段小写字母开头 */
const NS_SHAPE = /^[a-z][A-Za-z0-9]*(\.[A-Za-z0-9]+)+$/
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
  let dynamic = false
  let isI18n = false

  const lineOf = (pos) => sf.getLineAndCharacterOfPosition(pos).line + 1

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
        if (arg && ts.isStringLiteral(arg) && NS_SHAPE.test(arg.text)) {
          refs.push({ key: arg.text, line: lineOf(arg.getStart(sf)) })
        } else if (
          arg &&
          (ts.isTemplateExpression(arg) || ts.isNoSubstitutionTemplateLiteral(arg))
        ) {
          dynamic = true
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

  if (TEST_FILE.test(fileName)) return { refs: [], keys: [], dynamic: false }
  if (!isI18n) return { refs: [], keys: [], dynamic: false }
  return { refs, keys, dynamic }
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
  const used = []
  let dynamicFiles = 0

  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const full = join(dir, name)
      const st = readdirSync(dir, { withFileTypes: true }).find((d) => d.name === name)
      if (st.isDirectory()) {
        if (name === 'node_modules' || name === 'dist') continue
        walk(full)
      } else if (CODE_EXT.test(name)) {
        const { refs, keys, dynamic } = findRefs(readFileSync(full, 'utf-8'), name)
        if (dynamic) dynamicFiles += 1
        used.push(...keys)
        const rel = relative(REPO, full).split('\\').join('/')
        const hit = classify(refs, known, namespaces)
        for (const h of hit.missing) missing.push({ ...h, file: rel })
        for (const h of hit.unknownNs) unknownNs.push({ ...h, file: rel })
      }
    }
  }
  walk(SRC)
  return { missing, unknownNs, dynamicFiles, deadUpperBound: [...known].length - used.length }
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

  const { missing, unknownNs, dynamicFiles } = scan()
  if (!missing.length && !unknownNs.length) {
    const leaf = Object.keys(flatten(
      JSON.parse(readFileSync(join(SRC, 'i18n', WORDBOOK[0]), 'utf-8')),
    )).length
    console.log(
      `词库无缺失键、无失效命名空间引用（zh.json 扁平叶子键 ${leaf} 个）。` +
      `另有 ${dynamicFiles} 个文件使用动态键 t(\`...\`)，死键方向不在本门判定范围内` +
      `（静态枚举不到，报出来的是上界不是事实）。`,
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
