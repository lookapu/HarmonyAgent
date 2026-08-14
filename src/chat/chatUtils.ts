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

/** 渲染 Agent 文本：去掉残留的工具标记（工具调用已由 ToolRunGroup 折叠卡片展示，正文不重复显示）
 * 结束符容错：模型偶发把【TOOL|...】的】写成 ]} 或 ]，或整行漏写，一并清理；
 * 标记参数若被截断泄漏，后端 JSON 结构扫描（tools.rs mark_end_offset）为权威清理，此处仅实时渲染兜底 */
export function sanitizeToolMarkers(text: string): string {
  // 第一遍：清理完整标记（】/]} 结束，或标记独立成行自然到行尾/串尾）。
  // 注意不使用 `](?=\s|$)` 作为结束符：JSON 参数内 `] `（如 "arr[1] = 2"）会被误截断产生碎片
  text = text.replace(/【TOOL\|[^|】\n]+(?:\|[^】\n]*?)?(?:】|]}|(?=\n|$))/g, '')
  // 第二遍：清理残缺标记（流式截断/漏写结束符且后跟同行正文）：从【TOOL| 删到行尾。
  // 与后端 strip_tool_calls 的 None 分支一致：宁丢少量正文也不把代码碎片展示给用户
  return text.replace(/【TOOL\|[^\n]*/g, '')
}
