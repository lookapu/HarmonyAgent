/**
 * 布局探针：在无头 Chromium（默认本机 Edge）里量 P1-4 关心的**可计数布局量**。
 *
 * ## 它量什么，不量什么
 *
 * **量**：在 1100×720 视口下，有多少个列表项 / 消息块**完整可见**（不需滚动）。
 * **不量**：像素截图对比。理由见 `layout-fixtures.mjs` 的说明——
 * 字体渲染跨机器不一致，截图 diff 的阈值只能调宽到失去意义。
 *
 * ## 零新增依赖
 *
 * 用 Node 22 内置的 `WebSocket` 直连 Chromium DevTools Protocol。
 * 刻意不引 Playwright/Puppeteer：为一个「量几个数字」的需求装几百 MB 的浏览器驱动，
 * 换来的却是一个跨机器阈值永远在飘的门禁。
 *
 * 用法
 *   node scripts/layout-probe.mjs                 # 打印测量结果 JSON
 *   node scripts/layout-probe.mjs --update        # 写入 src/generated/layoutBaseline.json
 *   node scripts/layout-probe.mjs --browser <路径> # 指定浏览器
 */
import { spawn } from 'node:child_process'
import { createServer } from 'node:http'
import { readFileSync, existsSync, mkdtempSync, rmSync, writeFileSync, mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, dirname, extname, join, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { buildShim } from './layout-shim-build.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
export const REPO = resolve(HERE, '..')
const DIST = join(REPO, 'dist')
export const BASELINE = join(REPO, 'src', 'generated', 'layoutBaseline.json')

/** P1-4 的判据窗口。必须是「默认窗口」——判据是相对这个尺寸说的 */
export const VIEWPORT = { width: 1100, height: 720 }

const DEFAULT_BROWSERS = [
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
]

export function findBrowser(explicit) {
  if (explicit) {
    if (!existsSync(explicit)) throw new Error(`指定的浏览器不存在：${explicit}`)
    return explicit
  }
  const found = DEFAULT_BROWSERS.find((p) => existsSync(p))
  if (!found) {
    throw new Error(
      '找不到可用的 Chromium/Edge。\n' +
      'CI 的 windows-latest 预装 Edge；本机请用 --browser 指定路径。\n' +
      '**刻意不自动下载 Playwright 的浏览器**：为一个量布局的需求装几百 MB 驱动，\n' +
      '换来的是一个跨机器阈值永远在飘的门禁。',
    )
  }
  return found
}

const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.json': 'application/json',
  '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.png': 'image/png',
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function serveDist() {
  if (!existsSync(join(DIST, 'index.html'))) {
    throw new Error('dist/index.html 不存在，请先 npm run build')
  }
  return new Promise((res) => {
    const server = createServer((req, rq) => {
      const url = (req.url || '/').split('?')[0]
      let f = join(DIST, url === '/' ? 'index.html' : url)
      if (!existsSync(f) || !extname(f)) f = join(DIST, 'index.html')
      try {
        rq.writeHead(200, { 'Content-Type': MIME[extname(f)] || 'application/octet-stream' })
        rq.end(readFileSync(f))
      } catch { rq.writeHead(404); rq.end('not found') }
    })
    server.listen(0, '127.0.0.1', () => res({ server, port: server.address().port }))
  })
}

/**
 * 页面内执行的测量脚本。
 *
 * 每项都返回「**完整可见**」的数量：元素底边落在容器可视区内。
 * 判据是 P1-4 的「多显示约 15% 内容」，而「内容」就是这个计数。
 * 刻意不量像素高度——高度会随字体/DPR 漂，计数不会。
 *
 * ## 虚拟列表：总数必须由应用自己报
 *
 * 消息区是 TanStack Virtual，只挂载可视窗口附近的条目。
 * 用 `querySelectorAll` 数到的是**挂载数**，不是总数——30 条消息只挂 5 个是设计如此，
 * 把它当总数会让基线变成一个没人能解释的数字。所以总数读自
 * `[data-layout="message-virtual"]` 的 `data-total`（由 Home 写入），
 * 探针只负责校验「总数 ≥ 挂载数」这条不变量。
 */
const MEASURE = `(() => {
  const h = (v) => Math.round(v * 100) / 100;
  const pick = (sel) => document.querySelector(sel);
  const pickAll = (sel) => Array.from(document.querySelectorAll(sel));

  // 只数「完整可见」的：元素顶边与底边都落在容器可视区内。
  // 判据是 P1-4 的「多显示约 15% 内容」，而「内容」就是这个计数。
  function fullyVisible(items, container) {
    if (!container) return null;
    const cr = container.getBoundingClientRect();
    return items.filter((el) => {
      const r = el.getBoundingClientRect();
      return r.height > 0 && r.top >= cr.top - 0.5 && r.bottom <= cr.bottom + 0.5
    }).length;
  }

  // 定位不到时返回 null 而**不是 0**：0 会被读成「一行都显示不出来」，
  // 而真相是「探针没找到这个元素」。两者混在一起会让基线悄悄变成 0。
  const sessionBox = pick('[data-layout="session-list"]');
  const sessionItems = pickAll('[data-layout="session-item"]');
  const messageBox = pick('[data-layout="message-list"]');
  const virtualBox = pick('[data-layout="message-virtual"]');
  const messageItems = pickAll('[data-layout="message-item"]');
  const messageMsgs = messageItems.filter((el) => el.getAttribute('data-kind') === 'msg');

  // 总数由应用自己报；data-total 缺失时返回 null，**不拿挂载数顶替**
  const reportedTotal = virtualBox ? virtualBox.getAttribute('data-total') : null;
  const messageTotal = reportedTotal == null ? null : Number(reportedTotal);

  const heights = sessionItems.slice(0, 40).map((el) => h(el.getBoundingClientRect().height));
  const sorted = [...heights].sort((a, b) => a - b);

  return JSON.stringify({
    viewport: { w: window.innerWidth, h: window.innerHeight },
    // 会话列表**没有**虚拟滚动，querySelectorAll 数到的就是真总数
    sessionsFullyVisible: sessionBox ? fullyVisible(sessionItems, sessionBox) : null,
    sessionTotal: sessionItems.length,
    messagesFullyVisible: messageBox ? fullyVisible(messageMsgs, messageBox) : null,
    messageTotal,
    // 挂载数只作诊断；但「总数 < 挂载数」是自相矛盾，门禁必须能抓到
    messageMounted: messageItems.length,
    messageMsgMounted: messageMsgs.length,
    // 高度只用于解释变化（「为什么多了 3 行」），**不作门禁判据**：受字体与 DPR 影响
    sessionRowHeightMedian: sorted.length ? sorted[Math.floor(sorted.length / 2)] : null,
    sessionRowHeightMin: sorted.length ? sorted[0] : null,
    sessionRowHeightMax: sorted.length ? sorted[sorted.length - 1] : null,
    sessionListClientPx: sessionBox ? h(sessionBox.clientHeight) : null,
    sessionScrollablePx: sessionBox ? h(sessionBox.scrollHeight - sessionBox.clientHeight) : null,
    messageScrollablePx: messageBox ? h(messageBox.scrollHeight - messageBox.clientHeight) : null,
    // 页面级健康度：没渲染出来时这些会是 0 / 空
    rootTextLength: (document.body.innerText || '').length,
    hasErrorBoundary: /页面出现错误|Caught error/.test(document.body.innerText || ''),
  });
})()`

async function connect(cdpPort, tries = 60) {
  for (let i = 0; i < tries; i++) {
    await sleep(250)
    try {
      const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()
      const page = list.find((t) => t.type === 'page')
      if (page) return page
    } catch { /* 还没起来 */ }
  }
  throw new Error('浏览器 CDP 没起来')
}

export async function probe({
  browser: browserArg, viewport = VIEWPORT, shim, settleMs = 2500, screenshotPath, dump,
} = {}) {
  const exe = findBrowser(browserArg)
  // 每次重新打包 shim：多花 0.3s 换掉「预构建产物与 shim 源码脱节」这个真源
  const shimCode = shim ?? (await buildShim())
  const { server, port } = await serveDist()
  const profile = mkdtempSync(join(tmpdir(), 'layout-probe-'))
  const cdpPort = 9000 + Math.floor(Math.random() * 900)
  const proc = spawn(exe, [
    '--headless=new', `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-gpu',
    '--hide-scrollbars', '--force-device-scale-factor=1',
    '--disable-lcd-text', // 固定次像素抗锯齿，减少机器间差异
  ], { stdio: 'ignore' })

  let ws
  try {
    const target = await connect(cdpPort)
    ws = new WebSocket(target.webSocketDebuggerUrl)
    await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej })

    let id = 0
    const pending = new Map()
    const errors = []
    // React 的 ErrorBoundary 会把错误 **catch 住**再 console.error，
    // 所以 Runtime.exceptionThrown 抓不到——必须收 console 才能拿到堆栈。
    const consoleLines = []
    ws.onmessage = (ev) => {
      const m = JSON.parse(ev.data)
      if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id) }
      if (m.method === 'Runtime.exceptionThrown') {
        errors.push(m.params?.exceptionDetails?.exception?.description || 'unknown')
      }
      if (m.method === 'Runtime.consoleAPICalled') {
        const text = (m.params.args || [])
          .map((a) => a.value ?? a.description ?? `[${a.type}]`)
          .join(' ')
        consoleLines.push(text)
      }
    }
    const send = (method, params = {}) => new Promise((res) => {
      const n = ++id
      pending.set(n, res)
      ws.send(JSON.stringify({ id: n, method, params }))
    })

    await send('Page.enable')
    await send('Runtime.enable')
    // 记录**浏览器版本**而不是可执行文件路径：路径每台机器都不同，会让每次 --update
    // 都产生无谓的 git diff；而版本正是「同一台机器上数字却变了」时要排查的第一个变量。
    const ver = await send('Browser.getVersion')
    const browserVersion = ver.result?.product ?? null
    // **必须用 Emulation 而不是 --window-size**：headless 的窗口尺寸含边框，
    // 实测 --window-size=1100,720 时 innerHeight 只有 594。
    await send('Emulation.setDeviceMetricsOverride', {
      width: viewport.width, height: viewport.height, deviceScaleFactor: 1, mobile: false,
    })
    // 时区也要钉死：会话列表按**本地时间**分组（今天/昨天/本周），
    // formatTime 同理。UTC+8 的开发机与 UTC 的 runner 会把同一批 fixture
    // 分到不同的组，行高与可见行数随之改变——而两边都「没改任何代码」。
    await send('Emulation.setTimezoneOverride', { timezoneId: 'UTC' })
    // 必须在导航**之前**注入：应用脚本一加载就会调 invoke
    await send('Page.addScriptToEvaluateOnNewDocument', { source: shimCode })
    await send('Page.navigate', { url: `http://127.0.0.1:${port}/` })
    await sleep(settleMs)

    const r = await send('Runtime.evaluate', { expression: MEASURE, returnByValue: true })
    const raw = r.result?.result?.value
    if (!raw) throw new Error('测量脚本没返回结果（页面可能没加载完）')
    const measured = JSON.parse(raw)

    // --dump：任意表达式的一次性诊断通道。
    // 存在的理由是「想知道某个盒子的高度由哪几块构成」这类问题，
    // 过去每次都要手写一份 probe-cdp.mjs —— 那是重复造轮子，且用完就躺在仓库里。
    // 它不进基线、不参与判据。
    let dumped = null
    if (dump) {
      const dr = await send('Runtime.evaluate', { expression: dump, returnByValue: true })
      dumped = dr.result?.result?.value ?? null
    }

    // 落进 ErrorBoundary 时把正文打出来：只说「失败了」没法定位是哪条 fixture
    if (measured.hasErrorBoundary) {
      const t = await send('Runtime.evaluate', {
        expression: 'document.body.innerText.slice(0, 600)', returnByValue: true,
      })
      measured._errorPageText = t.result?.result?.value || ''
    }

    // 自洽不变量：应用报的总数不可能小于它自己挂载的条目数。
    // 违反只有两种可能——data-total 写错了，或 data-layout 钩子与渲染层脱节。
    // 两种都会让基线失去意义，所以**当场抛**，不在下游用「数量看着差不多」蒙过去。
    if (measured.messageTotal != null && measured.messageMounted > measured.messageTotal) {
      throw new Error(
        `自相矛盾：应用报 ${measured.messageTotal} 条，DOM 里却挂载了 ${measured.messageMounted} 个虚拟条目。\n` +
        '多半是 data-total 与 data-layout 钩子不同步（只改了一端）。',
      )
    }

    const diag = await send('Runtime.evaluate', {
      expression: 'JSON.stringify({ missing: window.__probe?.missing ?? [], called: (window.__probe?.called ?? []).length })',
      returnByValue: true,
    })
    const d = JSON.parse(diag.result?.result?.value || '{}')

    // 截图**只用于人工确认页面真的渲染出来了**，不进基线、不参与门禁判据。
    // 理由见 layout-fixtures.mjs：像素比对跨机器不可靠。
    // 但「数字看着合理」不等于「页面看着对」——4 行/40 这种比例必须眼见为实。
    let screenshot = null
    if (screenshotPath) {
      const shot = await send('Page.captureScreenshot', { format: 'png' })
      if (shot.result?.data) {
        mkdirSync(dirname(screenshotPath), { recursive: true })
        writeFileSync(screenshotPath, Buffer.from(shot.result.data, 'base64'))
        screenshot = screenshotPath
      }
    }

    return {
      viewport,
      browser: basename(exe),
      browserVersion,
      measured,
      missingFixtures: d.missing || [],
      invokedCommands: d.called || 0,
      pageErrors: errors.slice(0, 5),
      screenshot,
      dump: dumped,
      consoleErrors: consoleLines
        .filter((l) => /Error|error|Caught|undefined|null/.test(l))
        .slice(0, 4),
    }
  } finally {
    try { ws?.close() } catch { /* ignore */ }
    proc.kill()
    server.close()
    try { rmSync(profile, { recursive: true, force: true }) } catch { /* ignore */ }
  }
}

/** 门禁判据的指标名。顺序即检查顺序，也是 `--update` 报告的顺序。 */
export const JUDGE_METRICS = ['sessionsFullyVisible', 'messagesFullyVisible']

/** 从探针结果里抽出「本轮实测的判据数字」，供 scripts/check-density.py 判回归用 */
function countsOf(result) {
  return {
    sessionsFullyVisible: result.measured.sessionsFullyVisible,
    messagesFullyVisible: result.measured.messagesFullyVisible,
  }
}

/**
 * 造基线文件。
 *
 * ## 为什么 `measured` 与 `targets` 是两段
 *
 * `--update` 只刷新 `measured`；`targets` 是**人的承诺**，脚本不许自动改写。
 * 若让 `--update` 顺手把目标同步成实测值，这道门禁就永远绿——
 * 「一个永远绿的检查比没有检查更糟，它会让人以为这一类问题有人管」。
 *
 * 目标只许**显式**下调（`--accept-regression`），而那次下调会留在一个**文本文件**的
 * git diff 里，看得见、审得动。
 */
function toBaseline(result, targets, { accepted = [] } = {}) {
  return {
    _comment: [
      '本文件的 `measured` 段由 scripts/layout-probe.mjs 生成，**不要手改**。',
      '改 CSS 之后跑 `node scripts/layout-probe.mjs --update` 重新测量。',
      'scripts/check-density.py 负责把 `measured` 与 `targets` 对照。',
      '',
      '`targets` 是**人的承诺**，`--update` 不会改写它；',
      '要下调必须显式 `--accept-regression <指标>`，那一次下调会留在 git diff 里。',
      '',
      '计数（sessionsFullyVisible / messagesFullyVisible）是**门禁判据**；',
      '高度（*Height / *Scrollable）只用于解释变化，跨机器会飘，不作判据。',
    ].join('\n'),
    viewport: result.viewport,
    measured: {
      counts: {
        sessionsFullyVisible: result.measured.sessionsFullyVisible,
        sessionTotal: result.measured.sessionTotal,
        messagesFullyVisible: result.measured.messagesFullyVisible,
        messageTotal: result.measured.messageTotal,
      },
      // 诊断：解释「为什么多了一行」，不作判据
      diagnostic: {
        sessionRowHeightMedian: result.measured.sessionRowHeightMedian,
        sessionRowHeightMin: result.measured.sessionRowHeightMin,
        sessionRowHeightMax: result.measured.sessionRowHeightMax,
        sessionListClientPx: result.measured.sessionListClientPx,
        sessionScrollablePx: result.measured.sessionScrollablePx,
        messageScrollablePx: result.measured.messageScrollablePx,
        // 挂载数 ≠ 总数（虚拟列表）。留着它们，别人看到 5 vs 30 时才不会以为数据坏了
        messageMounted: result.measured.messageMounted,
        messageMsgMounted: result.measured.messageMsgMounted,
      },
    },
    targets,
    acceptedRegressions: accepted,
    probe: {
      // 只记文件名不记绝对路径：路径每台机器都不同，会让每次 --update 都产生
      // 一处无谓的 git diff，把真正要看的「数字变了」淹没在噪音里
      browser: result.browser,
      browserVersion: result.browserVersion,
      invokedCommands: result.invokedCommands,
      missingFixtures: result.missingFixtures,
    },
  }
}

/** 读旧基线里的 targets；没有或坏掉时返回 null（首次建基线） */
function readTargets() {
  if (!existsSync(BASELINE)) return null
  try {
    const old = JSON.parse(readFileSync(BASELINE, 'utf-8'))
    return old?.targets ?? null
  } catch {
    return null
  }
}

async function main() {
  const args = process.argv.slice(2)
  const update = args.includes('--update')
  const bi = args.indexOf('--browser')
  const si = args.indexOf('--screenshot')
  const ai = args.indexOf('--accept-regression')
  const di = args.indexOf('--dump')
  const oi = args.indexOf('--json-out')
  const accepted = ai >= 0 ? [args[ai + 1]] : []
  const result = await probe({
    browser: bi >= 0 ? args[bi + 1] : undefined,
    screenshotPath: si >= 0 ? args[si + 1] : undefined,
    dump: di >= 0 ? args[di + 1] : undefined,
  })

  // --json-out：把**本轮实测**写出去给门禁判回归。
  // 门禁不能只读入库的基线——CSS 改了但没人重跑探针时，那份文件还是旧的，
  // 于是门禁会对着过期数字报绿。**判据必须来自当场量出来的那一次。**
  if (oi >= 0) {
    mkdirSync(dirname(args[oi + 1]), { recursive: true })
    writeFileSync(
      args[oi + 1],
      JSON.stringify({
        viewport: result.viewport,
        counts: countsOf(result),
        diagnostics: {
          sessionTotal: result.measured.sessionTotal,
          messageTotal: result.measured.messageTotal,
          messageMounted: result.measured.messageMounted,
          sessionRowHeightMedian: result.measured.sessionRowHeightMedian,
        },
        missingFixtures: result.missingFixtures,
        hasErrorBoundary: result.measured.hasErrorBoundary,
      }, null, 2) + '\n',
      'utf-8',
    )
  }

  // --dump 在所有校验**之前**打印：页面崩进 ErrorBoundary 时恰恰最需要它，
  // 放在后面就永远看不到它最该被看到的那一次输出。
  if (result.dump != null) {
    console.log('--- dump ---')
    console.log(result.dump)
    console.log('------------')
  }

  if (result.measured.hasErrorBoundary) {
    console.error('✗ 页面落进了 ErrorBoundary——布局数据不可信，先修 fixture 或应用')
    if (result.measured._errorPageText) {
      console.error('页面正文：\n' + result.measured._errorPageText)
    }
    if (result.consoleErrors?.length) {
      console.error('\n应用侧堆栈（ErrorBoundary 打到 console 的）：')
      for (const l of result.consoleErrors) console.error(l.slice(0, 900))
    }
    if (result.pageErrors?.length) {
      console.error('\n未捕获异常：')
      for (const l of result.pageErrors) console.error(l.slice(0, 900))
    }
    process.exit(1)
  }
  if (result.missingFixtures.length) {
    console.error(`✗ 缺 ${result.missingFixtures.length} 个命令的 fixture：${result.missingFixtures.join(', ')}`)
    process.exit(1)
  }
  if (result.measured.sessionsFullyVisible === null) {
    console.error('✗ 定位不到会话列表（缺 data-layout="session-list"）——测量无从谈起')
    process.exit(1)
  }

  const prev = readTargets()
  // 首次建基线：把实测值立为目标
  const targets = { ...(prev ?? {}) }
  for (const m of JUDGE_METRICS) {
    if (targets[m] == null) targets[m] = result.measured[m]
  }

  if (update) {
    // 回归检测在**写盘之前**：把退步的数据 bless 成新基线，等于把门禁自己拆了
    const regressed = JUDGE_METRICS.filter(
      (m) => result.measured[m] != null && targets[m] != null && result.measured[m] < targets[m],
    ).filter((m) => !accepted.includes(m))

    if (regressed.length) {
      console.error('✗ 密度回归，拒绝写入基线：')
      for (const m of regressed) {
        console.error(`    ${m}：目标 ${targets[m]} → 实测 ${result.measured[m]}（少了 ${targets[m] - result.measured[m]}）`)
      }
      console.error('\n先修布局。确认要下调目标再显式执行：')
      for (const m of regressed) console.error(`    node scripts/layout-probe.mjs --update --accept-regression ${m}`)
      process.exit(1)
    }

    // 改进**自动抬高**目标、下调**必须显式**。这个不对称是刻意的：
    // 抬高是把成果锁住（安全），下调是承认退步（需要有人负责）。
    // 反过来做的话，--update 会顺手把目标同步成实测值，门禁就永远绿。
    for (const m of JUDGE_METRICS) {
      if (result.measured[m] == null) continue
      if (result.measured[m] > targets[m]) {
        console.log(`↻ 密度提升，自动抬高目标：${m} ${targets[m]} → ${result.measured[m]}`)
        targets[m] = result.measured[m]
      }
    }

    for (const m of accepted) {
      if (!JUDGE_METRICS.includes(m)) {
        console.error(`✗ --accept-regression 收到未知指标「${m}」，可选：${JUDGE_METRICS.join(' / ')}`)
        process.exit(1)
      }
      if (targets[m] === result.measured[m]) {
        console.error(`✗ ${m} 实测与目标相同（${result.measured[m]}），这次下调没有任何东西可下调`)
        process.exit(1)
      }
      console.log(`⚠ 已显式下调目标：${m} ${targets[m]} → ${result.measured[m]}`)
      targets[m] = result.measured[m]
    }

    mkdirSync(dirname(BASELINE), { recursive: true })
    writeFileSync(BASELINE, JSON.stringify(toBaseline(result, targets, { accepted }), null, 2) + '\n', 'utf-8')
    console.log(`已写入 ${relative(REPO, BASELINE)}`)
  }

  console.log(JSON.stringify(toBaseline(result, targets, { accepted }), null, 2))
}

// 入口判断必须用 pathToFileURL 而不是字符串拼 `file://${argv[1]}`：
// Windows 上那是 `file://H:/...`（两个斜杠），而 import.meta.url 是
// `file:///H:/...`（三个），拼出来永远不相等——脚本会**静默什么都不做**。
//
// argv[1] 也可能整个不存在（`node -e`、`node --input-type=module -e`）。
// 本文件导出 probe() 供复用，而「被 import」正是复用的主要方式——
// 没有这道判断时，一次普通 import 会在**模块加载期**抛
// ERR_INVALID_ARG_TYPE，把复用方还没写的一行代码变成报错原因。
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((e) => { console.error(String(e)); process.exit(1) })
}
