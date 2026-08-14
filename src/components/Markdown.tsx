import { createElement, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import remarkBreaks from 'remark-breaks'
import remarkMath from 'remark-math'
import rehypeKatex from 'rehype-katex'
import rehypeRaw from 'rehype-raw'
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize'
import hljs from 'highlight.js'
import { open as shellOpen } from '@tauri-apps/plugin-shell'
import { useThemeStore } from '../stores/themeStore'
import Icon from '../icons/Icon'
import 'katex/dist/katex.min.css'

// 常用语言注册（按需加载，避免全量包体积）
import rust from 'highlight.js/lib/languages/rust'
import groovy from 'highlight.js/lib/languages/groovy'
import properties from 'highlight.js/lib/languages/properties'
import bash from 'highlight.js/lib/languages/bash'
import shell from 'highlight.js/lib/languages/shell'
import xml from 'highlight.js/lib/languages/xml'
import json from 'highlight.js/lib/languages/json'
import typescript from 'highlight.js/lib/languages/typescript'
import javascript from 'highlight.js/lib/languages/javascript'
import css from 'highlight.js/lib/languages/css'
import sql from 'highlight.js/lib/languages/sql'
import python from 'highlight.js/lib/languages/python'
import java from 'highlight.js/lib/languages/java'
import kotlin from 'highlight.js/lib/languages/kotlin'
import go from 'highlight.js/lib/languages/go'
import c from 'highlight.js/lib/languages/c'
import cpp from 'highlight.js/lib/languages/cpp'
import csharp from 'highlight.js/lib/languages/csharp'
import markdown from 'highlight.js/lib/languages/markdown'
import yaml from 'highlight.js/lib/languages/yaml'
import ini from 'highlight.js/lib/languages/ini'
import diff from 'highlight.js/lib/languages/diff'
import dockerfile from 'highlight.js/lib/languages/dockerfile'
import plaintext from 'highlight.js/lib/languages/plaintext'

const languages: Record<string, unknown> = {
  rust, groovy, properties, bash, shell, xml, json, typescript, javascript, css, sql,
  python, java, kotlin, go, c, cpp, csharp, markdown, yaml, ini, diff, dockerfile,
  'docker': dockerfile, 'ts': typescript, 'js': javascript, 'py': python, 'yml': yaml, 'sh': shell, plaintext,
}
Object.entries(languages).forEach(([name, lang]) => {
  if (lang) hljs.registerLanguage(name, lang as never)
})

/** 语言展示名（角标用） */
function langLabel(lang: string): string {
  const map: Record<string, string> = { typescript: 'TS', javascript: 'JS', properties: 'INI', plaintext: 'TXT', dockerfile: 'Docker', yaml: 'YAML' }
  return map[lang] ?? lang.slice(0, 8)
}

/**
 * 识别代码首行的文件路径注释（AI 常用 // # <!-- -- 等开头标注文件名）。
 * 返回路径与剥离首行后的代码；无路径注释时 filePath 为空、bodyCode 为原文。
 * 仅当首行形如 "<注释符> <路径>" 且路径含扩展名或分隔符时才识别，避免误判普通注释。
 */
function extractFilePath(code: string): { filePath: string; bodyCode: string } {
  const firstNl = code.indexOf('\n')
  if (firstNl < 0) return { filePath: '', bodyCode: code }
  const first = code.slice(0, firstNl).trim()
  // 去掉常见注释前缀
  const stripped = first.replace(/^(?:\/\/|#|<!--|--|\/\*|\*|;)\s*/, '').replace(/-->$/, '').trim()
  // 看起来像路径：含 / 或 \，或形如 name.ext
  const looksPath = /[/\\]/.test(stripped) || /^[\w.-]+\.[a-zA-Z0-9]{1,6}$/.test(stripped)
  if (looksPath && stripped.length < 200 && !/\s/.test(stripped)) {
    return { filePath: stripped, bodyCode: code.slice(firstNl + 1) }
  }
  return { filePath: '', bodyCode: code }
}

/** 标题渲染工厂：总结/结论/注意事项等收尾关键词 → 强调色标题 */
function headingRender(level: 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6') {
  return function Heading({ children, ...props }: { children?: ReactNode }) {
    const text = extractText(children).trim()
    const key = /^(总结|结论|小结|摘要|最终结果|最终效果|下一步(计划|建议)?|后续(建议|计划)?|注意事项|关键点|关键要点|快速上手|快速开始)/.test(
      text.replace(/^[#\s]*/, ''),
    )
    return createElement(level, { ...props, className: key ? 'md-heading-key' : undefined }, children)
  }
}

/** rehype-sanitize 自定义白名单：默认基础上放行 kbd / 引用角标 sup 类名 / callout 类名 */
const sanitizeSchema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames ?? []), 'kbd'],
  attributes: {
    ...(defaultSchema.attributes ?? {}),
    sup: [...(defaultSchema.attributes?.sup ?? []), 'className'],
    kbd: ['className'],
    blockquote: [...(defaultSchema.attributes?.blockquote ?? []), 'className'],
  },
}

/**
 * 对话消息 Markdown 渲染：
 * GFM 表格/任务列表 + 单换行 + LaTeX（remark-math/rehype-katex）+ 原始 HTML 白名单（kbd/引用角标）
 * 代码块：折叠 / 行号 / 全屏 / 下载 / 语言标签 / 语法高亮（hljs 自管理）
 * Mermaid 图表（失败兜底显示原文）；图片 Lightbox 预览 + 加载失败占位
 */
export default function Markdown({
  children,
  className = '',
  onOpenFile,
  focusLine,
  selectedLines,
  onLineClick,
}: {
  children: string
  className?: string
  /** 点击代码块文件路径头时回调（相对路径），用于在项目中定位/引用该文件 */
  onOpenFile?: (path: string) => void
  /** 代码块高亮并滚动到的行号（1-based，文件预览场景） */
  focusLine?: number
  /** 代码块连续选区 [起, 止]（1-based，闭区间），范围内行整体高亮 */
  selectedLines?: [number, number]
  /** 点击代码块行号回调（含鼠标事件，用于 Shift 范围选择） */
  onLineClick?: (line: number, e: React.MouseEvent) => void
}) {
  const [lightbox, setLightbox] = useState<string | null>(null)
  // 引用角标预处理（跳过代码块区域）：[1] / [1,2] → <sup> 角标
  const content = useMemo(() => preprocessCitations(normalizeMarkdown(children)), [children])
  return (
    <div className={`md-body ${className}`}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks, remarkMath]}
        rehypePlugins={[
          [rehypeKatex, { throwOnError: false }],
          rehypeRaw,
          [rehypeSanitize, sanitizeSchema],
        ]}
        components={{
          /** 总结/结论/注意事项等收尾标题 → 强调色卡片式标题（各 AI 输出习惯差异较大，按关键词识别） */
          h1: headingRender('h1'),
          h2: headingRender('h2'),
          h3: headingRender('h3'),
          h4: headingRender('h4'),
          h5: headingRender('h5'),
          h6: headingRender('h6'),
          a({ href, children, ...props }) {
            if (!href || href.startsWith('#')) {
              return (
                <a href={href} {...props}>
                  {children}
                </a>
              )
            }
            return (
              <a
                href={href}
                title={href}
                onClick={(e) => {
                  e.preventDefault()
                  shellOpen(href).catch(() => window.open(href, '_blank', 'noopener'))
                }}
              >
                {children}
              </a>
            )
          },
          code({ className, children, ...props }) {
            const match = /language-([\w-]+)/.exec(className || '')
            const text = extractText(children)
            if (match) {
              const lang = match[1].toLowerCase()
              // Mermaid 图表单独渲染（失败兜底为代码块）
              if (lang === 'mermaid') {
                return <MermaidBlock code={text} />
              }
              // 目录树（text/plaintext/txt/tree 语言）→ 美化渲染
              if (['text', 'plaintext', 'txt', 'tree'].includes(lang) && isTreeText(text)) {
                return <TreeBlock code={text} />
              }
              return <CodeBlock lang={lang} code={text} onOpenFile={onOpenFile} focusLine={focusLine} selectedLines={selectedLines} onLineClick={onLineClick} />
            }
            // 无语言多行代码块：内容为目录树时也美化
            if (text.includes('\n') && isTreeText(text)) {
              return <TreeBlock code={text} />
            }
            // 无语言多行代码块：逐行渲染（HTML 会折叠 <code> 内的换行，导致多行内容挤成一行）
            if (text.includes('\n')) {
              return <CodeBlock lang="plaintext" code={text} onOpenFile={onOpenFile} focusLine={focusLine} selectedLines={selectedLines} onLineClick={onLineClick} />
            }
            return (
              <code className={className} {...props}>
                {children}
              </code>
            )
          },
          p({ children }) {
            // 纯目录树段落（无代码块包裹的裸树）→ 美化渲染
            const text = pText(children)
            if (isTreeText(text)) {
              return <TreeBlock code={text} />
            }
            return <p>{children}</p>
          },
          pre({ children }) {
            // 块级代码由自定义 code 组件渲染（避免 pre 嵌套 pre）
            return <>{children}</>
          },
          table({ children, ...props }) {
            return (
              <div className="md-table-wrap">
                <table {...props}>{children}</table>
              </div>
            )
          },
          input({ checked, ...props }) {
            // GFM 任务列表 checkbox（react-markdown 默认渲染 disabled input）
            return <input type="checkbox" checked={checked} readOnly {...props} />
          },
          img({ src, alt }) {
            return <SmartImage src={src ?? ''} alt={alt} onZoom={() => src && setLightbox(src)} />
          },
          kbd({ children }) {
            return <kbd className="md-kbd">{children}</kbd>
          },
          sup({ children, ...props }) {
            return <sup {...props}>{children}</sup>
          },
        }}
      >
        {content}
      </ReactMarkdown>
      {lightbox && <Lightbox src={lightbox} onClose={() => setLightbox(null)} />}
    </div>
  )
}

/**
 * 归一化各家 AI 的 Markdown 输出差异，使渲染更稳健美观（跳过围栏代码块/行内代码）：
 * 1. 非标准项目符号（• ● ▪ ◦ · ‣、全角 －、中文 、开头）→ 标准 "- "
 * 2. GitHub 风格 callout（> [!NOTE]/[!TIP]/[!IMPORTANT]/[!WARNING]/[!CAUTION]）→ 带类名 blockquote
 * 3. 裸 URL 不做处理（react-markdown 会自动链接）；统一过多连续空行为最多两行
 * 4. 移除标题前多余的 # 后无空格情况（"#标题" → "# 标题"）
 */
export function normalizeMarkdown(md: string): string {
  if (!md) return md
  // 按围栏代码块切分，奇数段为代码块内部，原样保留
  const fence = md.split(/(```[\s\S]*?```)/g)
  return fence
    .map((seg, i) => {
      if (i % 2 === 1) return seg
      return normalizeSegment(seg)
    })
    .join('')
}

function normalizeSegment(text: string): string {
  let out = text
  // 行内代码占位，避免内部符号被转换
  const inlineCodes: string[] = []
  out = out.replace(/`[^`\n]*`/g, (m) => {
    inlineCodes.push(m)
    return `\u0000ICODE${inlineCodes.length - 1}\u0000`
  })

  const lines = out.split('\n')
  const result: string[] = []
  let blankStreak = 0
  for (let raw of lines) {
    let line = raw
    // 标题：#后无空格 → 补空格（最多6级）
    line = line.replace(/^(#{1,6})(?=\S)/, '$1 ')

    // 非标准无序列表符号：行首可能有缩进，后跟 •●▪◦·‣◆◇►▸ 或 全角 －–—、ASCII -（排除 --- 水平线）、* + 紧接非空格
    // 统一为两个空格缩进 + "- "（保留原有缩进层级）
    line = line.replace(
      /^(\s*)(?:[•●▪◦·‣◆◇►▸]|[－–—*+]|-(?!-))(?:\s+|\s*(?=\S))/,
      (_m, indent: string) => {
        const depth = Math.min(3, Math.floor((indent as string).replace(/\t/g, '  ').length / 2))
        return `${'  '.repeat(depth)}- `
      },
    )
    // "1、" "1．" "1)" 中文序号 → 标准有序 "1. "（保留缩进）
    line = line.replace(/^(\s*)(\d{1,2})[、．)]\s*/, (_m, indent: string, n: string) => `${indent}${n}. `)

    result.push(line)

    if (line.trim() === '') {
      blankStreak++
    } else {
      blankStreak = 0
    }
  }
  out = result.join('\n')

  // 连续空行压缩为最多一个空行（= 两个换行）
  out = out.replace(/\n{3,}/g, '\n\n')

  // GitHub 风格 callout：> [!NOTE] 等 → blockquote 带类名
  out = out.replace(
    /^\s*>\s*\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\][^\n]*\n?((?:\s*>?[^\n]*\n?)*)/gim,
    (_m, kind: string, body: string) => {
      const cleaned = String(body)
        .split('\n')
        .map((l) => l.replace(/^\s*>\s?/, ''))
        .join('\n')
        .trim()
      return `<blockquote class="md-callout md-callout-${String(kind).toLowerCase()}"><p><strong>${kind}</strong></p>\n${cleaned}\n</blockquote>\n\n`
    },
  )

  // 还原行内代码
  out = out.replace(/\u0000ICODE(\d+)\u0000/g, (_m, i: string) => inlineCodes[Number(i)] ?? '')
  return out
}

/** 引用角标预处理：把 [1] / [1,2] 替换为 <sup>（跳过 ``` 代码块区域，避免误伤 URL/数字列表） */
export function preprocessCitations(md: string): string {
  const parts = md.split('```')
  return parts
    .map((part, i) => {
      if (i % 2 === 1) return part // 代码块内不处理
      return part.replace(/(^|[^0-9\]\w])\[(\d{1,2}(?:[-,，]\d{1,2})*)\](?![0-9])/g, '$1<sup class="citation-mark">$2</sup>')
    })
    .join('```')
}

/** 递归提取 ReactNode 中的纯文本（用于复制代码） */
function extractText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(extractText).join('')
  if (node && typeof node === 'object' && 'props' in node) {
    return extractText((node as { props: { children?: ReactNode } }).props.children)
  }
  return ''
}

/* ============ 目录树检测与解析 ============ */

/** 是否为目录树连接行（Unicode ├└│ 或 ASCII |-- +-- 风格） */
function isTreeLine(line: string): boolean {
  return /[├└│]/.test(line) || /[|+`]-{2}/.test(line)
}

/**
 * 是否整体为目录树文本：至少 2 行树行，且非树行不超过 1 个（根目录名/说明行）。
 * 普通文本/表格/diff 等不含树形连接符，不会误伤。
 */
export function isTreeText(text: string): boolean {
  const lines = text.split('\n').filter((l) => l.trim() !== '')
  if (lines.length < 2) return false
  let treeRows = 0
  let nonTree = 0
  lines.forEach((l) => {
    if (isTreeLine(l)) treeRows++
    else nonTree++
  })
  return treeRows >= 2 && nonTree <= 1
}

/** 解析树行：去掉行首连接符，得到名称与是否目录 */
function parseTreeLine(line: string): { name: string; isDir: boolean } {
  const name = line.replace(/^[\s│├└─|+`-]+/, '').trim()
  return { name, isDir: name.endsWith('/') }
}

/** 提取段落纯文本（br 元素还原为换行，用于裸树段落检测） */
function pText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(pText).join('')
  if (node && typeof node === 'object' && 'props' in node) {
    const el = node as { type?: unknown; props: { children?: ReactNode } }
    if (el.type === 'br') return '\n'
    return el.props.children ? pText(el.props.children) : ''
  }
  return ''
}

/* ============ 目录树：图标 + 目录高亮 + 折叠/复制（保留原始连线，等宽渲染） ============ */
function TreeBlock({ code }: { code: string }) {
  const [copied, setCopied] = useState(false)
  const [collapsed, setCollapsed] = useState(code.split('\n').length > COLLAPSE_THRESHOLD)
  const lines = useMemo(() => code.replace(/\n$/, '').split('\n'), [code])
  const shownLines = collapsed ? lines.slice(0, 10) : lines

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // 剪贴板不可用时静默失败
    }
  }

  return (
    <div className="md-tree">
      <div className="md-codeblock-head">
        <span className="md-codeblock-lang">
          <span className="md-codeblock-lang-dot" />
          目录结构
        </span>
        <div className="flex items-center gap-0.5">
          {lines.length > COLLAPSE_THRESHOLD && (
            <button type="button" className="md-codeblock-btn" onClick={() => setCollapsed((v) => !v)} title={collapsed ? '展开' : '折叠'}>
              <Icon name="chevron-right" size={13} className={collapsed ? '' : 'rotate-90 transition-transform'} />
              {collapsed ? `展开 (${lines.length} 行)` : '折叠'}
            </button>
          )}
          <button type="button" className="md-codeblock-btn" onClick={copy} title="复制">
            {copied ? (
              <span className="md-copied-ok">
                <Icon name="check" size={13} />已复制
              </span>
            ) : (
              <Icon name="copy" size={13} />
            )}
          </button>
        </div>
      </div>
      <div className="md-tree-body">
        {shownLines.map((line, i) => {
          const { name, isDir: dirLike } = parseTreeLine(line)
          // 首行为根目录名（无连接符前缀）时按目录渲染
          const isDir = i === 0 && !isTreeLine(line) ? true : dirLike
          return (
            <div className={`md-tree-line${isDir ? ' is-dir' : ''}`} key={i} title={name}>
              <span className="md-tree-glyph">
                <Icon name={isDir ? 'folder' : 'file'} size={12} />
              </span>
              <span className="md-tree-text">{line}</span>
            </div>
          )
        })}
      </div>
      {collapsed && lines.length > 10 && (
        <button type="button" className="md-codeblock-expand" onClick={() => setCollapsed(false)}>
          展开全部 {lines.length} 行
        </button>
      )}
    </div>
  )
}

/* ============ 图片：Lightbox 预览 + 加载失败占位 ============ */
function SmartImage({ src, alt, onZoom }: { src: string; alt?: string; onZoom: () => void }) {
  const [failed, setFailed] = useState(false)
  const [retryKey, setRetryKey] = useState(0)
  if (failed) {
    return (
      <div className="md-img-error">
        <Icon name="info" size={14} />
        <span>图片加载失败</span>
        <button type="button" onClick={() => { setFailed(false); setRetryKey((k) => k + 1) }}>
          重试
        </button>
      </div>
    )
  }
  return (
    <img
      key={retryKey}
      src={src}
      alt={alt}
      loading="lazy"
      className="md-img"
      onError={() => setFailed(true)}
      onClick={onZoom}
      title={alt || '点击放大'}
    />
  )
}

function Lightbox({ src, onClose }: { src: string; onClose: () => void }) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onClose])
  return (
    <div className="md-lightbox" onClick={onClose}>
      <img src={src} alt="" onClick={(e) => e.stopPropagation()} />
      <button type="button" className="md-lightbox-close" onClick={onClose} title="关闭 (Esc)">
        <Icon name="close" size={18} />
      </button>
    </div>
  )
}

/* ============ 代码块：折叠 / 行号 / 全屏 / 下载 / 复制 / 语言标签 ============ */
const COLLAPSE_THRESHOLD = 16 // 超过该行数自动折叠

function CodeBlock({
  lang,
  code,
  onOpenFile,
  focusLine,
  selectedLines,
  onLineClick,
}: {
  lang: string
  code: string
  onOpenFile?: (path: string) => void
  focusLine?: number
  selectedLines?: [number, number]
  onLineClick?: (line: number, e: React.MouseEvent) => void
}) {
  const [copied, setCopied] = useState(false)
  const lineCount = code.split('\n').length
  // 指定了 focusLine 或 selectedLines 且行号在范围内时，强制展开代码块以保证目标行可见
  const inRange = (n?: number) => n != null && n >= 1 && n <= lineCount
  const [collapsed, setCollapsed] = useState(
    lineCount > COLLAPSE_THRESHOLD && !inRange(focusLine) && !(selectedLines && (inRange(selectedLines[0]) || inRange(selectedLines[1]))),
  )
  // 选区/焦点行在折叠后变化时（如 Shift 点选），自动展开以露出目标行
  useEffect(() => {
    if (collapsed && (inRange(focusLine) || (selectedLines && (inRange(selectedLines[0]) || inRange(selectedLines[1]))))) {
      setCollapsed(false)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusLine, selectedLines])
  const [fullscreen, setFullscreen] = useState(false)
  // 首行路径注释识别：AI 常输出 "// src/foo.ts" / "# src/foo.py" / "<!-- src/x -->" 作为文件标识。
  // 识别后在头部展示路径，并从代码中剥离该行，避免污染语法高亮与复制内容。
  const { filePath, bodyCode } = useMemo(() => extractFilePath(code), [code])
  const highlighted = useMemo(() => {
    try {
      const langName = hljs.getLanguage(lang) ? lang : 'plaintext'
      return hljs.highlight(bodyCode, { language: langName, ignoreIllegals: true }).value
    } catch {
      return bodyCode.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    }
  }, [bodyCode, lang])
  const lines = useMemo(() => bodyCode.replace(/\n$/, '').split('\n'), [bodyCode])
  const shownLines = collapsed ? lines.slice(0, 10) : lines
  // diff 语言：按行首 + / - 标记增改行，高亮整行（绿/红底）
  const isDiff = lang === 'diff' || lang === 'patch'
  const lineKind = (line: string): 'add' | 'del' | 'ctx' | null => {
    if (!isDiff) return null
    if (line.startsWith('+') && !line.startsWith('+++')) return 'add'
    if (line.startsWith('-') && !line.startsWith('---')) return 'del'
    return 'ctx'
  }
  // shell/bash 输出中识别 error/warning 行（命令执行结果高亮）
  const isShell = ['bash', 'shell', 'sh'].includes(lang)
  const lineTone = (line: string): 'err' | 'warn' | null => {
    if (!isShell) return null
    const l = line.toLowerCase()
    if (/\b(error|failed|failure|exception|fatal)\b/.test(l)) return 'err'
    if (/\b(warning|warn|deprecated)\b/.test(l)) return 'warn'
    return null
  }

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(bodyCode)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // 剪贴板不可用时静默失败
    }
  }

  const download = () => {
    try {
      const blob = new Blob([bodyCode], { type: 'text/plain;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      const ext = filePath ? filePath.split(/[\\/]/).pop() ?? '' : ''
      a.download = ext || `code.${lang === 'plaintext' ? 'txt' : lang}`
      a.click()
      setTimeout(() => URL.revokeObjectURL(url), 1000)
    } catch {
      // 下载失败静默
    }
  }

  const toolbar = (extra?: ReactNode) => (
    <div className="md-codeblock-head">
      <span className="md-codeblock-lang">
        {filePath ? (
          <span className="md-codeblock-file" title={filePath}>
            {onOpenFile ? (
              <button
                type="button"
                onClick={() => onOpenFile(filePath)}
                className="md-codeblock-file-btn"
                title={`定位到 ${filePath}`}
              >
                <Icon name="file" size={11} className="shrink-0 opacity-70" />
                <span className="truncate">{filePath}</span>
                <Icon name="folder" size={10} className="opacity-50 shrink-0" />
              </button>
            ) : (
              <>
                <Icon name="file" size={11} className="shrink-0 opacity-70" />
                <span className="truncate">{filePath}</span>
              </>
            )}
          </span>
        ) : (
          <>
            <span className="md-codeblock-lang-dot" />
            {langLabel(lang)}
          </>
        )}
      </span>
      <div className="flex items-center gap-0.5">
        {lines.length > COLLAPSE_THRESHOLD && (
          <button type="button" className="md-codeblock-btn" onClick={() => setCollapsed((v) => !v)} title={collapsed ? '展开' : '折叠'}>
            <Icon name="chevron-right" size={13} className={collapsed ? '' : 'rotate-90 transition-transform'} />
            {collapsed ? `展开 (${lines.length} 行)` : '折叠'}
          </button>
        )}
        <button type="button" className="md-codeblock-btn" onClick={download} title="下载代码">
          <Icon name="download" size={13} />
        </button>
        <button type="button" className="md-codeblock-btn" onClick={() => setFullscreen((v) => !v)} title={fullscreen ? '退出全屏' : '全屏'}>
          <Icon name="chevron-right" size={13} className="rotate-[-45deg]" />
        </button>
        <button type="button" className="md-codeblock-btn" onClick={copy} title="复制代码">
          {copied ? (
            <span className="md-copied-ok">
              <Icon name="check" size={13} />已复制
            </span>
          ) : (
            <Icon name="copy" size={13} />
          )}
        </button>
      </div>
      {extra}
    </div>
  )

  const body = (
    <pre className="md-codeblock-pre">
      {shownLines.map((line, i) => {
        const lineNo = i + 1
        const k = lineKind(line)
        const tone = k ? null : lineTone(line)
        const isFocus = focusLine === lineNo
        const inSel =
          selectedLines && lineNo >= selectedLines[0] && lineNo <= selectedLines[1]
        const cls = `md-code-line${k ? ` md-diff-line md-diff-${k}` : ''}${tone ? ` md-line-${tone}` : ''}${isFocus ? ' md-code-line-focus' : ''}${inSel ? ' md-code-line-selected' : ''}`
        const lineNoEl = onLineClick ? (
          <button
            type="button"
            className="md-code-line-no md-code-line-no-btn"
            onClick={(e) => onLineClick(lineNo, e)}
            title="点击选择该行，Shift+点击选择范围"
          >
            {lineNo}
          </button>
        ) : onOpenFile && filePath ? (
          <button
            type="button"
            className="md-code-line-no md-code-line-no-btn"
            title={`定位到 ${filePath}:${lineNo}`}
            onClick={() => onOpenFile(`${filePath}:${lineNo}`)}
          >
            {lineNo}
          </button>
        ) : (
          <span className="md-code-line-no">{lineNo}</span>
        )
        return (
          <div className={cls} key={i} data-line={lineNo}>
            {lineNoEl}
            <span
              className="md-code-line-content"
              dangerouslySetInnerHTML={{
                __html: i === shownLines.length - 1 && line === '' ? '<br/>' : line === '' ? ' ' : highlighted.split('\n')[i] ?? '',
              }}
            />
          </div>
        )
      })}
    </pre>
  )

  if (fullscreen) {
    return (
      <div className="md-codeblock md-codeblock-fullscreen">
        {toolbar(
          <button type="button" className="md-codeblock-btn ml-auto" onClick={() => setFullscreen(false)} title="退出全屏 (Esc)">
            退出全屏
          </button>,
        )}
        {body}
      </div>
    )
  }

  return (
    <div className="md-codeblock">
      {toolbar()}
      {body}
      {collapsed && lines.length > 10 && (
        <button type="button" className="md-codeblock-expand" onClick={() => setCollapsed(false)}>
          展开全部 {lines.length} 行
        </button>
      )}
    </div>
  )
}

/* ============ Mermaid 图表（异步渲染 + 失败兜底） ============ */
let mermaidSeq = 0
/** 小地图固定宽度（px），高度按 SVG 纵横比自适应 */
const MINIMAP_W = 160

function MermaidBlock({ code }: { code: string }) {
  const theme = useThemeStore((s) => s.theme)
  const [svg, setSvg] = useState('')
  const [error, setError] = useState<string | null>(null)
  const idRef = useRef(`md-mermaid-${++mermaidSeq}-${Math.random().toString(36).slice(2)}`)
  const containerRef = useRef<HTMLDivElement>(null)
  // 缩放/平移状态：滚轮缩放，拖拽平移
  const [zoom, setZoom] = useState(1)
  const [pan, setPan] = useState({ x: 0, y: 0 })
  const dragRef = useRef<{ x: number; y: number; px: number; py: number } | null>(null)
  const [fullscreen, setFullscreen] = useState(false)
  // 小地图导航（仅全屏）：追踪视口与原始 SVG 尺寸，渲染缩略图 + 可拖拽视口框
  const viewportRef = useRef<HTMLDivElement>(null)
  const [svgSize, setSvgSize] = useState({ w: 0, h: 0 })
  const [vpSize, setVpSize] = useState({ w: 0, h: 0 })
  const minimapDragRef = useRef<
    | { kind: 'pan'; rect: DOMRect }
    | { kind: 'box'; lastX: number; lastY: number }
    | null
  >(null)

  const resetView = () => {
    setZoom(1)
    setPan({ x: 0, y: 0 })
  }

  // 全屏时按 Esc 退出
  useEffect(() => {
    if (!fullscreen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setFullscreen(false)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [fullscreen])

  useEffect(() => {
    let cancelled = false
    setSvg('')
    setError(null)
    import('mermaid')
      .then(({ default: mermaid }) => {
        const isDark = theme === 'dark'
        // 使用 base 主题 + themeVariables 对齐应用调色板（浅色用 --bg-card/--accent，深色同步反转），
        // 避免 mermaid default/dark 主题自带的纯白/深蓝背景与应用不协调。
        mermaid.initialize({
          startOnLoad: false,
          theme: 'base',
          securityLevel: 'loose',
          fontFamily: 'inherit',
          themeVariables: isDark
            ? {
                dark: true,
                background: '#1a1d26',
                primaryColor: '#262b3a',
                primaryBorderColor: '#3b4256',
                primaryTextColor: '#e6e8ee',
                secondaryColor: '#1f2433',
                tertiaryColor: '#151821',
                lineColor: '#6b7488',
                textColor: '#c9d1d9',
                mainBkg: '#262b3a',
                nodeBkg: '#262b3a',
                nodeBorder: '#6b7488',
                clusterBkg: '#1f2433',
                clusterBorder: '#3b4256',
                edgeLabelBackground: '#1f2433',
                labelTextColor: '#e6e8ee',
                titleColor: '#ffffff',
                actorBkg: '#262b3a',
                actorBorder: '#6b7488',
                actorTextColor: '#e6e8ee',
                signalColor: '#c9d1d9',
                signalTextColor: '#c9d1d9',
                noteBkgColor: '#3d3520',
                noteBorderColor: '#6b5a2e',
                noteTextColor: '#f0e6c8',
              }
            : {
                dark: false,
                background: '#ffffff',
                primaryColor: '#eef2ff',
                primaryBorderColor: '#6366f1',
                primaryTextColor: '#1e2330',
                secondaryColor: '#f5f7fb',
                tertiaryColor: '#ffffff',
                lineColor: '#8a93a6',
                textColor: '#2a2f3a',
                mainBkg: '#eef2ff',
                nodeBkg: '#eef2ff',
                nodeBorder: '#6366f1',
                clusterBkg: '#f5f7fb',
                clusterBorder: '#d8dde8',
                edgeLabelBackground: '#ffffff',
                labelTextColor: '#1e2330',
                titleColor: '#11141a',
                actorBkg: '#eef2ff',
                actorBorder: '#6366f1',
                actorTextColor: '#1e2330',
                signalColor: '#2a2f3a',
                signalTextColor: '#2a2f3a',
                noteBkgColor: '#fff8d6',
                noteBorderColor: '#d9b441',
                noteTextColor: '#5c4a12',
              },
        })
        return mermaid.render(idRef.current, code).then(({ svg: rendered }) => {
          if (!cancelled) {
            setSvg(rendered)
          }
        })
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [code, theme])

  // SVG 渲染后记录其原始尺寸（getBoundingClientRect 在未缩放状态下测量，供小地图比例计算）
  useEffect(() => {
    if (!svg) return
    const node = containerRef.current?.querySelector('svg') as SVGSVGElement | null
    if (!node) return
    // 优先用 svg width/height 属性/视盒，回退到 getBBox
    const w = node.viewBox?.baseVal?.width || node.getBoundingClientRect().width
    const h = node.viewBox?.baseVal?.height || node.getBoundingClientRect().height
    setSvgSize({ w, h })
  }, [svg])

  // 全屏时监听视口（遮罩）尺寸，用于小地图视口框
  useEffect(() => {
    if (!fullscreen) return
    const measure = () => {
      const el = viewportRef.current
      if (!el) return
      setVpSize({ w: el.clientWidth, h: el.clientHeight })
    }
    measure()
    window.addEventListener('resize', measure)
    const ro = new ResizeObserver(measure)
    if (viewportRef.current) ro.observe(viewportRef.current)
    return () => {
      window.removeEventListener('resize', measure)
      ro.disconnect()
    }
  }, [fullscreen, svg])

  /**
   * 小地图交互有两种：
   * 1) 在缩略图背景上按下/拖拽：让该点居中于主视图（瞬移 + 跟随）
   * 2) 在视口框上按下/拖拽：按小地图位移增量平移视角（保持点击点在框内不跳动）
   */
  const onMinimapDown = (e: React.MouseEvent) => {
    if (e.button !== 0 || svgSize.w === 0) return
    e.stopPropagation()
    e.preventDefault()
    const mapEl = e.currentTarget as HTMLElement
    const rect = mapEl.getBoundingClientRect()
    const onBox = (e.target as HTMLElement).classList.contains('md-mermaid-minimap-vp')
    if (onBox) {
      minimapDragRef.current = { kind: 'box', lastX: e.clientX, lastY: e.clientY }
      // 框拖拽挂全局监听，鼠标移出小地图也能继续拖
      const onMove = (ev: MouseEvent) => handleBoxDrag(ev.clientX, ev.clientY)
      const onUp = () => {
        minimapDragRef.current = null
        window.removeEventListener('mousemove', onMove)
        window.removeEventListener('mouseup', onUp)
      }
      window.addEventListener('mousemove', onMove)
      window.addEventListener('mouseup', onUp)
    } else {
      minimapDragRef.current = { kind: 'pan', rect }
      moveMinimapTo(e.clientX, e.clientY, rect)
    }
  }
  const onMinimapMove = (e: React.MouseEvent) => {
    const d = minimapDragRef.current
    if (!d || d.kind !== 'pan') return
    moveMinimapTo(e.clientX, e.clientY, d.rect)
  }
  const onMinimapUp = () => {
    // pan 模式在移出小地图时可能仍需跟随，这里仅清理；box 模式用全局监听自行清理
    const d = minimapDragRef.current
    if (d?.kind === 'pan') minimapDragRef.current = null
  }
  /** 框拖拽：小地图位移增量 → 主视图平移 */
  const handleBoxDrag = (clientX: number, clientY: number) => {
    const d = minimapDragRef.current
    if (!d || d.kind !== 'box') return
    const scale = MINIMAP_W / svgSize.w
    const dx = (clientX - d.lastX) / scale
    const dy = (clientY - d.lastY) / (scale * (svgSize.h / svgSize.w || 1))
    d.lastX = clientX
    d.lastY = clientY
    setPan((p) => ({ x: p.x - dx * zoom, y: p.y - dy * zoom }))
  }
  /** 背景平移：小地图坐标 → 原始 SVG 坐标 → 让该点居中于视口 */
  const moveMinimapTo = (clientX: number, clientY: number, rect: DOMRect) => {
    const viewport = viewportRef.current
    if (!viewport || svgSize.w === 0 || svgSize.h === 0) return
    const scale = MINIMAP_W / svgSize.w
    const mapH = scale * svgSize.h
    const sx = (clientX - rect.left) / scale
    const sy = (clientY - rect.top) / (mapH / svgSize.h)
    setPan({
      x: viewport.clientWidth / 2 - sx * zoom,
      y: viewport.clientHeight / 2 - sy * zoom,
    })
  }

  if (error) {
    return (
      <div className="md-mermaid-error">
        <div className="md-mermaid-error-head">
          <Icon name="info" size={13} />
          <span>图表渲染失败</span>
        </div>
        <CodeBlock lang="mermaid" code={code} />
      </div>
    )
  }
  if (!svg) {
    return (
      <div className="md-mermaid-loading">
        <span className="w-3 h-3 rounded-full border-2 border-[var(--accent)] border-t-transparent animate-spin" />
        图表渲染中…
      </div>
    )
  }

  /** 下载 SVG：序列化当前 svg 节点并触发下载 */
  const downloadSvg = () => {
    const container = containerRef.current
    if (!container) return
    const node = container.querySelector('svg')
    if (!node) return
    const clone = node.cloneNode(true) as SVGElement
    clone.setAttribute('xmlns', 'http://www.w3.org/2000/svg')
    const source = new XMLSerializer().serializeToString(clone)
    const blob = new Blob([`<?xml version="1.0" encoding="UTF-8"?>\n${source}`], { type: 'image/svg+xml;charset=utf-8' })
    triggerDownload(blob, 'diagram.svg')
  }

  /** 下载 PNG：把 SVG 绘制到 canvas 后导出（含白底，兼容透明查看器） */
  const downloadPng = () => {
    const container = containerRef.current
    if (!container) return
    const node = container.querySelector('svg')
    if (!node) return
    const rect = node.getBoundingClientRect()
    const w = Math.ceil(rect.width)
    const h = Math.ceil(rect.height)
    const clone = node.cloneNode(true) as SVGElement
    clone.setAttribute('xmlns', 'http://www.w3.org/2000/svg')
    clone.setAttribute('width', String(w))
    clone.setAttribute('height', String(h))
    const source = new XMLSerializer().serializeToString(clone)
    const svgBlob = new Blob([source], { type: 'image/svg+xml;charset=utf-8' })
    const url = URL.createObjectURL(svgBlob)
    const img = new Image()
    img.onload = () => {
      const scale = 2
      const canvas = document.createElement('canvas')
      canvas.width = w * scale
      canvas.height = h * scale
      const ctx = canvas.getContext('2d')
      if (!ctx) return
      ctx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue('--bg-card').trim() || '#ffffff'
      ctx.fillRect(0, 0, canvas.width, canvas.height)
      ctx.scale(scale, scale)
      ctx.drawImage(img, 0, 0, w, h)
      URL.revokeObjectURL(url)
      canvas.toBlob((blob) => {
        if (blob) triggerDownload(blob, 'diagram.png')
      }, 'image/png')
    }
    img.onerror = () => URL.revokeObjectURL(url)
    img.src = url
  }

  const onWheel = (e: React.WheelEvent) => {
    if (!e.ctrlKey && !e.metaKey) return
    e.preventDefault()
    const delta = e.deltaY > 0 ? -0.1 : 0.1
    setZoom((z) => Math.max(0.3, Math.min(4, z + delta)))
  }
  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return
    dragRef.current = { x: e.clientX, y: e.clientY, px: pan.x, py: pan.y }
  }
  const onMouseMove = (e: React.MouseEvent) => {
    if (!dragRef.current) return
    setPan({
      x: dragRef.current.px + (e.clientX - dragRef.current.x),
      y: dragRef.current.py + (e.clientY - dragRef.current.y),
    })
  }
  const onMouseUp = () => {
    dragRef.current = null
  }

  const chart = (
    <div
      ref={containerRef}
      className={`md-mermaid${fullscreen ? ' md-mermaid-fullscreen-body' : ''}`}
      onWheel={onWheel}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onMouseLeave={onMouseUp}
      style={{
        cursor: dragRef.current ? 'grabbing' : 'grab',
        ['--mermaid-zoom' as string]: zoom,
        ['--mermaid-tx' as string]: `${pan.x}px`,
        ['--mermaid-ty' as string]: `${pan.y}px`,
      }}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  )

  const toolbar = (
    <div className="md-mermaid-toolbar">
      <button onClick={() => setZoom((z) => Math.min(4, +(z + 0.2).toFixed(2)))} title="放大">
        <Icon name="plus" size={12} />
      </button>
      <button onClick={() => setZoom((z) => Math.max(0.3, +(z - 0.2).toFixed(2)))} title="缩小" className="md-mermaid-zoom-btn">
        −
      </button>
      <button onClick={resetView} title="重置缩放" className="md-mermaid-zoom-pct">
        {Math.round(zoom * 100)}%
      </button>
      <button onClick={() => setFullscreen((v) => !v)} title={fullscreen ? '退出全屏' : '全屏'}>
        <Icon name={fullscreen ? 'close' : 'panel'} size={12} />
      </button>
      <span className="md-mermaid-toolbar-sep" />
      <button onClick={downloadPng} title="下载 PNG">
        <Icon name="file" size={12} />
        PNG
      </button>
      <button onClick={downloadSvg} title="下载 SVG">
        <Icon name="download" size={12} />
        SVG
      </button>
    </div>
  )

  // 小地图视口框：把当前可见区域（原始 SVG 坐标）映射到小地图坐标。
  // 主视图 transform-origin: center，缩放中心即 SVG 中心 (cx,cy)；
  // 可见区域左上角在原始坐标系为 (cx - panX/zoom, cy - panY/zoom)。
  const mapScale = svgSize.w ? MINIMAP_W / svgSize.w : 0
  const mapH = mapScale * svgSize.h
  const cx = svgSize.w / 2
  const cy = svgSize.h / 2
  const vpL = (cx - pan.x / zoom) * mapScale
  const vpT = (cy - pan.y / zoom) * mapScale
  const vpW = (vpSize.w / zoom) * mapScale
  const vpH = (vpSize.h / zoom) * mapScale

  const minimap = (
    <div
      className="md-mermaid-minimap"
      onMouseDown={onMinimapDown}
      onMouseMove={onMinimapMove}
      onMouseUp={onMinimapUp}
      onMouseLeave={onMinimapUp}
    >
      <div
        className="md-mermaid-minimap-img"
        style={{ width: MINIMAP_W, height: mapH || 'auto' }}
        dangerouslySetInnerHTML={{ __html: svg }}
      />
      {mapScale > 0 && (
        <div
          className="md-mermaid-minimap-vp"
          style={{ left: vpL, top: vpT, width: vpW, height: vpH }}
        />
      )}
    </div>
  )

  if (fullscreen) {
    return (
      <div
        className="md-mermaid-fullscreen"
        ref={viewportRef}
        onClick={(e) => e.target === e.currentTarget && setFullscreen(false)}
      >
        <div
          className="md-mermaid-fullscreen-inner"
          style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`, transformOrigin: 'center center' }}
        >
          {chart}
        </div>
        {minimap}
        <div className="md-mermaid-fullscreen-bar">{toolbar}</div>
      </div>
    )
  }

  return (
    <div className="md-mermaid-wrap">
      {chart}
      {toolbar}
    </div>
  )
}

/** 触发 Blob 下载 */
function triggerDownload(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
