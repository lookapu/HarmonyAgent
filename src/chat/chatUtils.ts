/** 任务/工具耗时格式化：mm:ss（超 1 小时 h:mm:ss），模块级供工具卡复用 */
export function fmtElapsed(sec: number) {
  const s = Math.max(0, Math.floor(sec))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const rest = String(s % 60).padStart(2, '0')
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${rest}` : `${m}:${rest}`
}

/** 恢复划词选区高亮。快照 Range 的边界节点可能因 DOM 更新失效：Chromium 会把失效边界
 * 归一化到最近的存活节点（选区收缩），此时改用保存的完整文本在容器内重建 Range；
 * 容器本身也被重建时，退化为在所有消息正文中按文本搜索。 */
export function restoreSelectionRange(r: Range | null, text?: string, container?: Node | null): void {
  if (!r) return
  try {
    const sel = window.getSelection()
    if (!sel) return
    // live Range 归一化检测：DOM 重建后文本变短 → 按文本重建端点
    if (text && r.toString() !== text) {
      const rebuilt = rebuildRangeByText(text, container)
      if (rebuilt) {
        sel.removeAllRanges()
        sel.addRange(rebuilt)
        return
      }
    }
    sel.removeAllRanges()
    sel.addRange(r)
  } catch {
    // 快照完全失效时静默跳过
  }
}

/** 按文本在容器内重建选区 Range（容器失效时在文档级 .md-body 中查找包含该文本的容器） */
function rebuildRangeByText(text: string, container?: Node | null): Range | null {
  const candidates: Node[] = []
  const alive = container && document.contains(container) ? container : null
  if (alive) candidates.push(alive)
  else {
    // 容器已重建：找第一个文本包含目标的正文容器（跨格式选区文本一般足够独特）
    document.querySelectorAll('.md-body').forEach((el) => {
      if (el.textContent?.includes(text)) candidates.push(el)
    })
  }
  if (candidates.length === 0) candidates.push(document.body)
  for (const root of candidates) {
    const nodes: Text[] = []
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
    while (walker.nextNode()) {
      const n = walker.currentNode as Text
      if (n.textContent && n.textContent.length > 0) nodes.push(n)
    }
    if (nodes.length === 0) continue
    const full = nodes.map((n) => n.textContent!).join('')
    const start = full.indexOf(text)
    if (start < 0) continue
    const range = document.createRange()
    range.setStart(...locate(nodes, start))
    range.setEnd(...locate(nodes, start + text.length))
    return range
  }
  return null
}

/** 把全局字符偏移映射回 (文本节点, 节点内偏移) */
function locate(nodes: Text[], globalOffset: number): [Node, number] {
  let acc = 0
  for (const n of nodes) {
    const len = n.textContent!.length
    if (globalOffset <= acc + len) return [n, globalOffset - acc]
    acc += len
  }
  const last = nodes[nodes.length - 1]
  return [last, last.textContent!.length]
}

/** 渲染 Agent 文本：去掉残留的工具标记（工具调用已由 ToolRunGroup 折叠卡片展示，正文不重复显示）。
 * 采用字符扫描器（移植自 Rust 端 strip_tool_calls/mark_end_offset），可跨行识别 JSON 参数边界，
 * 避免正则方案在多行参数（如 edit_file 的 old/new 含换行）时把后续行的 {"path":...、}、】 泄漏成正文碎片。
 * 结束符容错：】 / ]} / ] / 漏写结束符但 JSON 完整；JSON 残缺（流式截断）时丢弃该标记及其后全部内容。 */
const TOOL_MARK_START = '【TOOL|'
const TOOL_MARK_END = '】'

export function sanitizeToolMarkers(text: string): string {
  let out = ''
  let rest = text
  while (true) {
    const start = rest.indexOf(TOOL_MARK_START)
    if (start < 0) {
      out += rest
      break
    }
    out += rest.slice(0, start)
    const after = rest.slice(start + TOOL_MARK_START.length)
    const boundary = findMarkerEnd(after)
    if (boundary == null) {
      // 参数残缺且无任何结束符（流式截断）：丢弃标记及其后全部内容（与 Rust None 分支一致）
      break
    }
    rest = after.slice(boundary)
  }
  return out
}

/** 在 【TOOL| 之后的文本中定位标记结束位置（返回相对 after 的偏移）。
 * 先按 JSON 结构定位参数末尾，再识别结束符 】 > ]} > ]；JSON 完整但漏写结束符时以 JSON 末尾为界；
 * JSON 无法识别时回退到首个 】 / ]} / 换行。完全无法定位返回 null（整段丢弃）。 */
function findMarkerEnd(after: string): number | null {
  // 跳过工具名段，找第一个 | 作为参数起点（允许跨行）
  const pipe = after.indexOf('|')
  const paramFrom = pipe >= 0 ? pipe + 1 : 0
  let i = paramFrom
  while (i < after.length && /\s/.test(after[i])) i++
  if (i < after.length) {
    const jsonEnd = scanJsonValue(after, i)
    if (jsonEnd > i) {
      let p = jsonEnd
      // 结束符常被模型放到下一行；允许跨空白寻找，但未命中结束符时仍从
      // jsonEnd 返回，以保留原始正文换行。
      while (p < after.length && /\s/.test(after[p])) p++
      // 结束符变体：】 / ]} / ]
      if (after.startsWith(TOOL_MARK_END, p)) return p + TOOL_MARK_END.length
      if (after.startsWith(']}', p)) return p + 2
      if (after[p] === ']') return p + 1
      // 容错：JSON 后又跟一段 |...】（模型多写了一段尾注），吞到结束符/换行
      if (after[p] === '|') {
        const rest = after.slice(p)
        // 常见残缺尾标是 `|]}`；只吞掉尾标本身，保留其后的同一行正文。
        if (rest.startsWith('|]}')) return p + 3
        const e = rest.indexOf(TOOL_MARK_END)
        if (e >= 0) return p + e + TOOL_MARK_END.length
        const nl = rest.indexOf('\n')
        if (nl >= 0) return p + nl + 1
        return after.length
      }
      // JSON 完整但漏写结束符：以 JSON 末尾为界，保留其后正文
      return jsonEnd
    }
  }
  // 回退：非完整 JSON（残缺/纯文本）时按结束符/换行定位
  const e1 = after.indexOf(TOOL_MARK_END)
  if (e1 >= 0) return e1 + TOOL_MARK_END.length
  const e2 = after.indexOf(']}')
  if (e2 >= 0) return e2 + 2
  const nl = after.indexOf('\n')
  if (nl >= 0) return nl + 1
  return null
}

/** 从 pos 起扫描一个 JSON 值（对象/数组/字符串/字面量/数字），返回值结束后的偏移；无法识别返回 pos。
 * 字符串内的括号/引号按内容跳过，深度上限 64 防异常输入。 */
function scanJsonValue(s: string, pos: number): number {
  let i = pos
  const ch = s[i]
  if (ch === '{' || ch === '[') {
    let depth = 0
    while (i < s.length) {
      const c = s[i]
      if (c === '"') {
        const next = scanJsonString(s, i)
        // 未闭合字符串返回原位置；若继续会在同一个引号上无限循环并冻结 UI。
        if (next <= i) return pos
        i = next
        continue
      }
      if (c === '{' || c === '[') {
        depth++
        if (depth > 64) return pos
      } else if (c === '}' || c === ']') {
        depth--
        if (depth === 0) return i + 1
      }
      i++
    }
    return pos // 未闭合
  }
  if (ch === '"') return scanJsonString(s, pos)
  if (ch === 't' || ch === 'f' || ch === 'n') {
    const m = /^(true|false|null)/.exec(s.slice(pos))
    return m ? pos + m[0].length : pos
  }
  if (ch === '-' || (ch >= '0' && ch <= '9')) {
    let j = pos + 1
    while (j < s.length && /[-0-9.eE+]/.test(s[j])) j++
    return j
  }
  return pos
}

function scanJsonString(s: string, pos: number): number {
  let i = pos + 1
  while (i < s.length) {
    if (s[i] === '\\') {
      i += 2
      continue
    }
    if (s[i] === '"') return i + 1
    i++
  }
  return pos
}
