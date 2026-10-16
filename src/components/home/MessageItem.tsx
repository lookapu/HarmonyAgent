/**
 * 消息项渲染（类 Claude / Qoder 风格）
 *
 * 从 Home.tsx 搬出（原 L7628-8409）。**纯代码移动，逻辑未改。**
 * 23 个 props，但全部经 Home 传入，不依赖 Home 的任何局部状态，也没有反向依赖，
 * 因此可以整块搬走而不改动调用方。
 *
 * 三条已核实的不变量（改动前先确认不违反）：
 * 1. `EMPTY_VERSIONS` 必须是**模块级稳定引用**：在 MessageItem 内每次渲染新建
 *    数组会让子组件 memo 永远判定 props 变化，长会话下白重渲。
 * 2. `convId` 取自 `message.conversation_id`，**不是**从 store 反查当前会话——
 *    否则历史会话里的消息会 pin 到错误的会话上。
 * 3. 任何新增的外部依赖都必须是模块级 import；一旦引用 Home 组件体内的局部
 *    变量，搬出就会变成隐式耦合。**tsc 的 `Cannot find name` 是这里唯一的网**
 *    （本组件无渲染测试）。
 */

import { saveKnowledgeFromText } from '../../api/knowledge'
import { setConversationContextPin, type ChatMessage } from '../../api/project'
import { fmtElapsed, sanitizeToolMarkers } from '../../chat/chatUtils'
import { ModifiedFilesCard, ThinkingBlock, ThumbDownIcon, ThumbUpIcon } from '../../chat/components/messageBlocks'
import Markdown from '../../components/Markdown'
import { Button } from '../../components/ui/Button'
import { Field, TextArea } from '../../components/ui/Field'
import { IconButton } from '../../components/ui/IconButton'
import Icon from '../../icons/Icon'
import { useNotificationStore } from '../../stores/notificationStore'
import { PIN_MAX_PER_CONV, usePinStore } from '../../stores/pinStore'
import { useProjectStore } from '../../stores/projectStore'
import { useRatingStore } from '../../stores/ratingStore'
import { memo, useCallback, useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

/* ============ 消息（无气泡，Claude/Qoder 风格） ============ */
/** 无版本历史时的稳定空数组引用：避免在 MessageItem 内每次渲染创建新数组破坏子组件 memo */
const EMPTY_VERSIONS: ChatMessage[] = []
export const MessageItem = memo(function MessageItem({
  message,
  time,
  userMessageId,
  isLastAssistant,
  onRegenerate,
  onBranch,
  onRate,
  onDislike,
  onOpenVersions,
  onSpeak,
  speaking,
  onEditMessage,
  onDeleteMessage,
  confirmDeleteMsgId,
  projectPath,
  highlighted,
  onOpenFile,
  onQuoteMessage,
  onLocateMessage,
  onForkFrom,
  onCopyId,
  shortId,
  copiedId,
}: {
  message: ChatMessage
  time: string
  userMessageId: string
  isLastAssistant?: boolean
  onRegenerate?: () => void
  /** 从该 user 消息分支重生成（保留旧回复为版本历史） */
  onBranch?: (message: ChatMessage) => void
  onRate: (messageId: string, feedback: 'like' | 'dislike' | 'neutral', reason?: string) => void
  onDislike: (messageId: string) => void
  onOpenVersions: (message: ChatMessage, userMessageId: string) => void
  onSpeak: (messageId: string, text: string) => void
  speaking: boolean
  onEditMessage?: (message: ChatMessage) => void
  onDeleteMessage?: (message: ChatMessage) => void
  confirmDeleteMsgId?: string | null
  /** 项目路径（变更审查用；非 git 项目时为 undefined，保持只读列表） */
  projectPath?: string
  /** 搜索命中高亮：消息卡片闪烁背景 3 秒 */
  highlighted?: boolean
  /** 代码块文件路径点击：在项目中定位/引用该文件 */
  onOpenFile?: (path: string) => void
  /** 引用该消息到输入框（Quote） */
  onQuoteMessage?: (message: ChatMessage) => void
  /** 点击消息引用标签：定位并高亮被引用消息 */
  onLocateMessage?: (msgId: string) => void
  /** 从该 user 消息派生新会话（Fork：复制至此的消息，原会话不动） */
  onForkFrom?: (message: ChatMessage) => void
  /** 复制 ID 到剪贴板（排查问题用） */
  onCopyId?: (id: string) => void
  shortId: (id: string) => string
  copiedId: string | null
}) {
  const { t } = useTranslation()
  // 按消息粒度订阅：整表订阅会在任意一条消息点赞/版本更新时触发全部历史消息重渲染
  const feedback = useProjectStore((s) => s.feedbackMap[message.id])
  const versions = useProjectStore((s) => s.versionMap[userMessageId] ?? EMPTY_VERSIONS)
  const { role, content, reasoning, model } = message
  const [copied, setCopied] = useState(false)
  // 右键菜单：x/y 定位
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number } | null>(null)

  // 右键菜单：x/y + 菜单项动作（直接挂在 onContextMenu）
  const onContextMenu = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      setCtxMenu({ x: e.clientX, y: e.clientY })
    },
    [],
  )
  // 点击其他位置/滚动/失焦时关闭菜单
  useEffect(() => {
    if (!ctxMenu) return
    const close = () => setCtxMenu(null)
    window.addEventListener('click', close)
    window.addEventListener('scroll', close, true)
    return () => {
      window.removeEventListener('click', close)
      window.removeEventListener('scroll', close, true)
    }
  }, [ctxMenu])
  // 超长回复折叠：超过 2 万字符时默认收起，仅渲染纯文本摘要（避免单条消息数千行拖慢列表渲染与滚动）
  const [expanded, setExpanded] = useState(false)
  // “记住这次修复”弹窗：把当前对话里的错误+解法沉淀为知识库条目
  const [rememberOpen, setRememberOpen] = useState(false)
  const [rememberSaving, setRememberSaving] = useState(false)
  const [rememberForm, setRememberForm] = useState({ title: '', error_text: '', fix: '' })
  // 分支展开面板：对话内切换预览历史版本（预览为临时视图，不落库）
  const [branchOpen, setBranchOpen] = useState(false)
  const [previewVersionId, setPreviewVersionId] = useState<string | null>(null)

  // 本回复修改的文件列表（ChatGPT 式折叠卡片；edit_file/write_file 收集，相对路径）
  const modifiedFiles = useMemo(() => {
    if (!message.modified_files_json) return []
    try {
      const v = JSON.parse(message.modified_files_json)
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []
    } catch {
      return []
    }
  }, [message.modified_files_json])

  // 本条 user 消息的 @ 引用列表（references_json，气泡下方标签展示）；
  // "msg:{id}" 前缀条目为消息引用（Quote 功能），与文件引用分开渲染
  const { fileRefs, quotedMsgIds } = useMemo(() => {
    const fileRefs: string[] = []
    const quotedMsgIds: string[] = []
    if (!message.references_json) return { fileRefs, quotedMsgIds }
    try {
      const v = JSON.parse(message.references_json)
      if (!Array.isArray(v)) return { fileRefs, quotedMsgIds }
      for (const x of v) {
        if (typeof x !== 'string') continue
        if (x.startsWith('msg:')) quotedMsgIds.push(x.slice(4))
        else fileRefs.push(x)
      }
    } catch {
      /* 忽略损坏数据 */
    }
    return { fileRefs, quotedMsgIds }
  }, [message.references_json])

  // 注：role === 'tool' 的消息已在 renderItems 中合并为 ToolRunGroup 折叠组，不会进入本组件

  const isUser = role === 'user'
  const previewVersion = previewVersionId ? versions.find((v) => v.id === previewVersionId) ?? null : null
  const displayContent = previewVersion ? previewVersion.content : content
  const displayReasoning = previewVersion ? (previewVersion.reasoning ?? undefined) : reasoning
  const isLong = displayContent.length > 20000
  // 缓存 sanitizeToolMarkers 结果：长文本正则替换是 O(n) 开销，memoise 避免 MessageItem 因无关
  // props 变化（如 speaking/highlighted/confirmDeleteMsgId）重渲染时重复执行
  const sanitizedContent = useMemo(() => sanitizeToolMarkers(displayContent), [displayContent])

  /** 复制消息内容（带 1.5s 成功反馈） */
  const copyMessage = async () => {
    try {
      await navigator.clipboard.writeText(displayContent)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // 剪贴板不可用时静默失败
    }
  }

  const openRemember = () => {
    // 默认把助手回复的纯文本作为“修复方法”，标题/错误现象留空让用户补充
    const plainFix = displayContent
      .replace(/```[\s\S]*?```/g, ' ')
      .replace(/[#*`>_~-]/g, ' ')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 600)
    setRememberForm({ title: '', error_text: '', fix: plainFix })
    setRememberOpen(true)
  }

  const saveRemember = async () => {
    if (!rememberForm.error_text.trim()) {
      alert(t('knowledge.rememberNeedError'))
      return
    }
    setRememberSaving(true)
    try {
      const pid = useProjectStore.getState().currentProject?.id ?? null
      await saveKnowledgeFromText(
        {
          title: rememberForm.title || undefined,
          error_text: rememberForm.error_text,
          fix: rememberForm.fix,
        },
        pid,
      )
      setRememberOpen(false)
    } catch (e) {
      alert(String(e))
    } finally {
      setRememberSaving(false)
    }
  }

  if (isUser) {
    const queued = message.queued === 1
    return (
      <div
        data-msg-id={message.id}
        onContextMenu={onContextMenu}
        className={`flex justify-end gap-3 group msg-row ${highlighted ? 'msg-highlight' : ''}`}
      >
        <div className="max-w-[80%] rounded-2xl rounded-tr-md bg-[var(--bg-secondary)] px-4 py-2 transition-colors">
          <div className="flex items-center gap-2 mb-1">
            <span className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)]">{t('home.you')}</span>
            <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] group-hover:text-[var(--text-secondary)] transition-colors">{time}</span>
            {/* 消息 ID：默认隐藏，悬浮时显示（排查问题用） */}
            {onCopyId && (
              <button
                onClick={() => onCopyId(message.id)}
                className="font-mono text-[length:var(--app-text-micro)] px-1 py-px rounded text-[var(--text-muted)] hover:text-[var(--accent)] transition-colors opacity-0 group-hover:opacity-100"
                title={`${t('home.msgId')}: ${message.id}\n${t('home.clickToCopy')}`}
              >
                {copiedId === message.id ? <Icon name="check" size={8} className="text-[var(--success)]" /> : '#'}
                {shortId(message.id)}
              </button>
            )}
            {queued && (
              <span className="text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded-md bg-[var(--accent)]/10 text-[var(--accent)]">
                {message.agent_owned === 1 ? t('home.queuedAgentLabel') : t('home.queuedLabel')}
              </span>
            )}
            {/* @ 引用标签（references_json 落库展示） */}
            {fileRefs.length > 0 && (
              <span
                className="flex items-center gap-1 text-[length:var(--app-text-2xs)] text-[var(--text-muted)] max-w-52 overflow-hidden"
                title={fileRefs.join('\n')}
              >
                <Icon name="file" size={10} className="shrink-0" />
                <span className="truncate">{fileRefs.join(', ')}</span>
              </span>
            )}
            {/* 消息引用标签（Quote 落库 msg:{id}；点击定位被引用消息） */}
            {quotedMsgIds.length > 0 && onLocateMessage && quotedMsgIds.map((mid) => (
              <button
                key={mid}
                onClick={() => onLocateMessage(mid)}
                className="flex items-center gap-1 text-[length:var(--app-text-2xs)] text-[var(--accent)] hover:text-[var(--accent-hover)] max-w-40"
                title={t('home.locateQuoted')}
              >
                <Icon name="quote" size={10} className="shrink-0" />
                <span className="truncate">{t('home.quotedMsgLabel')}</span>
              </button>
            ))}
            <div className="ml-auto flex items-center gap-0.5 opacity-0 group-hover:opacity-100 max-md:opacity-100 transition-opacity">
              {onQuoteMessage && (
                <button
                  onClick={() => onQuoteMessage(message)}
                  className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] rounded-md px-1.5 py-0.5 flex items-center gap-0.5"
                  title={t('home.quoteMessage')}
                >
                  <Icon name="quote" size={11} />{t('home.quoteMessage')}
                </button>
              )}
              {onForkFrom && (
                <button
                  onClick={() => onForkFrom(message)}
                  className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] rounded-md px-1.5 py-0.5 flex items-center gap-0.5"
                  title={t('home.forkFromHere')}
                >
                  <Icon name="git-branch" size={11} />{t('home.forkFromHere')}
                </button>
              )}
              <button
                onClick={copyMessage}
                className={`text-[length:var(--app-text-2xs)] rounded-md px-1.5 py-0.5 flex items-center gap-0.5 ${
                  copied ? 'text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'
                }`}
                title={t('home.copyMessage')}
              >
                {copied ? <><Icon name="check" size={11} />{t('home.copied')}</> : <><Icon name="copy" size={11} />{t('home.copyMessage')}</>}
              </button>
              {onEditMessage && (
                <button
                  onClick={() => onEditMessage(message)}
                  className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] rounded-md px-1.5 py-0.5"
                  title={t('home.editMessage')}
                >
                  {t('home.editMessage')}
                </button>
              )}
              {onDeleteMessage && (
                <button
                  onClick={() => onDeleteMessage(message)}
                  className={`text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded-md transition-[color,background-color,border-color,box-shadow] ${
                    confirmDeleteMsgId === message.id
                      ? 'text-white bg-[var(--danger)] shadow-[0_0_0_3px_var(--danger-50)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'
                  }`}
                  title={
                    confirmDeleteMsgId === message.id
                      ? t('home.deleteMessageConfirm')
                      : t('home.deleteMessage')
                  }
                >
                  {confirmDeleteMsgId === message.id ? t('home.deleteMessageConfirm') : t('home.deleteMessage')}
                </button>
              )}
              {onBranch && !queued && (
                <button
                  onClick={() => onBranch(message)}
                  className="text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] flex items-center gap-0.5"
                  title={t('home.branchRegenerate')}
                >
                  <Icon name="git-branch" size={10} />
                  {t('home.branchRegenerate')}
                </button>
              )}
            </div>
          </div>
          <div className="text-sm break-words leading-relaxed">
            {/* 超长消息折叠（粘贴大段日志等）：与助手分支同规则 */}
            {isLong && !expanded ? (
              <div>
                <pre className="whitespace-pre-wrap break-all font-mono text-[length:var(--app-text-xs)] leading-relaxed text-[var(--text-secondary)] max-h-48 overflow-y-auto">
                  {content.slice(0, 2000)}
                </pre>
                <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)]">…</span>
                <button
                  onClick={() => setExpanded(true)}
                  className="ml-1 text-[length:var(--app-text-xs)] text-[var(--accent)] hover:underline"
                >
                  {t('home.expandFullMessage', { count: content.length.toLocaleString() })}
                </button>
              </div>
            ) : (
              <Markdown onOpenFile={onOpenFile}>{content}</Markdown>
            )}
          </div>
        </div>
        <div className="w-7 h-7 shrink-0 rounded-lg modern-card border-[var(--border)] flex items-center justify-center text-[length:var(--app-text-xs)] font-medium text-[var(--text-secondary)]">
          {t('home.you').charAt(0)}
        </div>
      </div>
    )
  }

  return (
    <div
      data-msg-id={message.id}
      onContextMenu={onContextMenu}
      className={`flex gap-3 group msg-row ${highlighted ? 'msg-highlight' : ''}`}
    >
      <div className="w-6 h-6 rounded-full bg-[var(--bg-hover)] flex items-center justify-center shrink-0 mt-0.5">
        <Icon name="spark" size={12} className="text-[var(--text-muted)]" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 mb-1">
          <span className="text-[length:var(--app-text-sm)] font-medium text-[var(--text-primary)]">{t('home.agent')}</span>
          <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] group-hover:text-[var(--text-secondary)] transition-colors">{time}</span>
          {/* 次要元信息：默认隐藏，悬浮消息时显示，保持清爽 */}
          <div className="flex items-center gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
            {onCopyId && (
              <button
                onClick={() => onCopyId(message.id)}
                className="font-mono text-[length:var(--app-text-micro)] px-1 py-px rounded text-[var(--text-muted)] hover:text-[var(--accent)] transition-colors"
                title={`${t('home.msgId')}: ${message.id}\n${t('home.clickToCopy')}`}
              >
                {copiedId === message.id ? <Icon name="check" size={8} className="text-[var(--success)]" /> : '#'}
                {shortId(message.id)}
              </button>
            )}
            {model && <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">{model}</span>}
            {message.duration_ms != null && message.duration_ms > 0 && (
              <span
                className="text-[length:var(--app-text-2xs)] tabular-nums text-[var(--text-muted)]"
                title={t('home.replyDurationHint')}
              >
                {fmtElapsed(message.duration_ms / 1000)}
              </span>
            )}
            {(message.tokens_in != null || message.tokens_out != null) && (
              <span
                className="text-[length:var(--app-text-2xs)] tabular-nums text-[var(--text-muted)]"
                title={`${t('home.tokenHint')}：${t('home.tokenIn')} ${message.tokens_in ?? 0} / ${t('home.tokenOut')} ${message.tokens_out ?? 0}`}
              >
                ↑{(message.tokens_in ?? 0).toLocaleString()} ↓{(message.tokens_out ?? 0).toLocaleString()}
              </span>
            )}
          </div>
        </div>
        {previewVersion && (
          <div className="flex items-center gap-2 mb-1.5">
            <span className="text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded-md bg-[var(--warning)]/12 text-[var(--warning)] flex items-center gap-1">
              <Icon name="git-branch" size={10} />
              {t('home.branchPreviewBadge')} · {new Date(previewVersion.created_at * 1000).toLocaleString()}
            </span>
            <button onClick={() => setPreviewVersionId(null)} className="text-[length:var(--app-text-2xs)] text-[var(--accent)] hover:underline">
              {t('home.branchBackToMain')}
            </button>
          </div>
        )}
        {displayReasoning && <ThinkingBlock content={displayReasoning} />}
        <div className="text-sm break-words leading-relaxed text-[var(--text-primary)]">
          {isLong && !expanded ? (
            <div>
              <pre className="whitespace-pre-wrap break-all font-mono text-[length:var(--app-text-xs)] leading-relaxed text-[var(--text-secondary)] max-h-48 overflow-y-auto">
                {displayContent.slice(0, 2000)}
              </pre>
              <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)]">…</span>
              <button
                onClick={() => setExpanded(true)}
                className="ml-1 text-[length:var(--app-text-xs)] text-[var(--accent)] hover:underline"
              >
                {t('home.expandFullMessage', { count: displayContent.length.toLocaleString() })}
              </button>
            </div>
          ) : (
            <Markdown onOpenFile={onOpenFile}>{sanitizedContent}</Markdown>
          )}
        </div>
        {modifiedFiles.length > 0 && <ModifiedFilesCard files={modifiedFiles} projectPath={projectPath} />}
        {/* 分支切换面板（对话内预览历史版本，点击版本直接切换显示） */}
        {branchOpen && versions.length > 0 && (
          <div className="mt-2 rounded-xl border border-[var(--accent)]/25 bg-[var(--accent-soft)]/40 p-2 animate-fade-in-up">
            <div className="flex items-center gap-1.5 flex-wrap">
              <button
                onClick={() => setPreviewVersionId(null)}
                className={`px-2.5 py-1 rounded-lg text-[length:var(--app-text-xs)] border transition-colors ${
                  previewVersionId === null
                    ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]'
                    : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--text-muted)]'
                }`}
              >
                {t('home.versionCurrent')}
              </button>
              {[...versions].reverse().map((v) => (
                <button
                  key={v.id}
                  onClick={() => setPreviewVersionId(v.id)}
                  className={`px-2.5 py-1 rounded-lg text-[length:var(--app-text-xs)] border transition-colors ${
                    previewVersionId === v.id
                      ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]'
                      : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--text-muted)]'
                  }`}
                >
                  {t('home.versionLabel', { n: new Date(v.created_at * 1000).toLocaleString() })}
                </button>
              ))}
              <IconButton
                icon="close"
                label={t('home.close')}
                iconSize={12}
                className="ml-auto"
                onClick={() => setBranchOpen(false)}
              />
              <button
                onClick={() => onOpenVersions(message, userMessageId)}
                className="px-2.5 py-1 rounded-lg text-[length:var(--app-text-xs)] border border-[var(--border)] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent)] transition-colors"
              >
                {t('home.branchCompareDiff')}
              </button>
            </div>
          </div>
        )}
        {/* 操作栏：复制 / 引用 / 重新生成 / 点赞 / 点踩 / 朗读 / 版本对比 */}
        <div className="flex items-center gap-0.5 mt-1.5 opacity-0 group-hover:opacity-100 max-md:opacity-100 transition-opacity">
          <IconButton
            icon={copied ? 'check' : 'copy'}
            label={t('home.copyMessage')}
            iconSize={13}
            onClick={copyMessage}
          />
          {onQuoteMessage && (
            <IconButton
              icon="quote"
              label={t('home.quoteMessage')}
              iconSize={13}
              onClick={() => onQuoteMessage(message)}
            />
          )}
          <IconButton
            icon="lightbulb"
            label={t('knowledge.rememberThisFix')}
            iconSize={13}
            onClick={openRemember}
          />
          {isLastAssistant && onRegenerate && (
            <IconButton
              icon="refresh"
              label={t('home.regenerate')}
              iconSize={13}
              onClick={onRegenerate}
            />
          )}
          <button
            onClick={() => onRate(message.id, feedback?.feedback === 'like' ? 'neutral' : 'like')}
            className={`p-1 rounded-md transition-colors ${feedback?.feedback === 'like' ? 'text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'}`}
            title={t('home.like')}
          >
            <ThumbUpIcon filled={feedback?.feedback === 'like'} />
          </button>
          <button
            onClick={() =>
              feedback?.feedback === 'dislike'
                ? onRate(message.id, 'neutral')
                : onDislike(message.id)
            }
            className={`p-1 rounded-md transition-colors ${feedback?.feedback === 'dislike' ? 'text-[var(--danger)]' : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'}`}
            title={t('home.dislike')}
          >
            <ThumbDownIcon filled={feedback?.feedback === 'dislike'} />
          </button>
          <button
            onClick={() => onSpeak(message.id, displayContent)}
            className={`p-1 rounded-md transition-colors ${speaking ? 'text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'}`}
            aria-label={speaking ? t('home.stopSpeak') : t('home.speak')}
            title={speaking ? t('home.stopSpeak') : t('home.speak')}
          >
            <Icon name="headphones" size={13} />
          </button>
          {versions.length > 0 && (
            <button
              onClick={() => setBranchOpen((v) => !v)}
              className={`p-1 rounded-md transition-colors text-[length:var(--app-text-2xs)] flex items-center gap-1 ${
                branchOpen
                  ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                  : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'
              }`}
              aria-label={t('home.viewVersions')}
              title={t('home.viewVersions')}
            >
              <Icon name="git-branch" size={12} />
              {versions.length}
            </button>
          )}
          {onDeleteMessage && (
            <button
              onClick={() => onDeleteMessage(message)}
              className={`p-1 rounded-md transition-[color,background-color,border-color,box-shadow] ${confirmDeleteMsgId === message.id ? 'bg-[var(--danger)] text-white shadow-[0_0_0_3px_var(--danger-50)]' : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'}`}
              aria-label={confirmDeleteMsgId === message.id ? t('home.deleteMessageConfirm') : t('home.deleteMessage')}
              title={confirmDeleteMsgId === message.id ? t('home.deleteMessageConfirm') : t('home.deleteMessage')}
            >
              <Icon name="delete" size={13} white={confirmDeleteMsgId === message.id} />
            </button>
          )}
        </div>
      </div>

      {rememberOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
          onClick={() => setRememberOpen(false)}
        >
          <div
            className="w-[480px] max-w-[92vw] rounded-xl modern-card border-[var(--border)] shadow-2xl p-4 space-y-3"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-[length:var(--app-text-base)] font-semibold">{t('knowledge.rememberTitle')}</h3>
            <p className="text-[length:var(--app-text-xs)] text-[var(--text-muted)]">{t('knowledge.rememberHint')}</p>
            <Field
              fieldSize="md"
              label={t('knowledge.entryTitle')}
              value={rememberForm.title}
              onChange={(e) => setRememberForm({ ...rememberForm, title: e.target.value })}
              placeholder={t('knowledge.rememberTitlePh')}
            />
            <TextArea
              fieldSize="md"
              label={t('knowledge.rememberError')}
              value={rememberForm.error_text}
              onChange={(e) => setRememberForm({ ...rememberForm, error_text: e.target.value })}
              rows={4}
              placeholder={t('knowledge.rememberErrorPh')}
            />
            <TextArea
              fieldSize="md"
              label={t('knowledge.fix')}
              value={rememberForm.fix}
              onChange={(e) => setRememberForm({ ...rememberForm, fix: e.target.value })}
              rows={5}
            />
            <div className="flex justify-end gap-2 pt-1">
              <Button variant="secondary" size="md" onClick={() => setRememberOpen(false)}>
                {t('mcp.cancel')}
              </Button>
              <Button variant="primary" size="md" loading={rememberSaving} onClick={saveRemember}>
                {t('knowledge.save')}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 右键菜单：复制 / 重新生成（仅 assistant 最后一条）/ 编辑（仅 user）/ 删除 */}
      {ctxMenu && (
        <div
          role="menu"
          onClick={(e) => e.stopPropagation()}
          className="fixed z-[var(--app-z-popover)] min-w-[180px] glass-card rounded-lg py-1 animate-modal-in text-[length:var(--app-text-sm)]"
          style={{ left: ctxMenu.x, top: ctxMenu.y }}
        >
          <button
            onClick={() => {
              copyMessage()
              setCtxMenu(null)
            }}
            className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name={copied ? 'check' : 'copy'} size={13} />
            {copied ? t('home.copied') : t('home.copyMessage')}
          </button>
          {message.role === 'user' && onEditMessage && (
            <button
              onClick={() => {
                onEditMessage(message)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="edit" size={13} />
              {t('home.edit')}
            </button>
          )}
          {message.role === 'user' && onDeleteMessage && (
            <button
              onClick={() => {
                onDeleteMessage(message)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--danger)] hover:bg-[var(--danger-50)] transition-colors"
            >
              <Icon name="delete" size={13} />
              {t('home.delete')}
            </button>
          )}
          {message.role === 'assistant' && isLastAssistant && onRegenerate && (
            <button
              onClick={() => {
                onRegenerate()
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="refresh" size={13} />
              {t('home.regenerate')}
            </button>
          )}
          {message.role === 'assistant' && onBranch && (
            <button
              onClick={() => {
                onBranch(message)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="git-branch" size={13} />
              {t('home.forkFromHere')}
            </button>
          )}
          {/* Pin 到会话顶部，同时持久化为 Context V2 权威上下文。 */}
          <button
            onClick={() => {
              const convId = message.conversation_id
              const isP = usePinStore.getState().isPinned(convId, message.id)
              usePinStore.getState().toggle(convId, message.id)
              void setConversationContextPin({
                conversation_id: convId,
                pin_kind: 'message',
                source_ref: `message:${message.id}`,
                label: message.role,
                content: message.content,
                pinned: !isP,
              }).catch(() => {})
              // toast 反馈
              useNotificationStore.getState().push({
                tone: isP ? 'info' : 'success',
                title: isP ? t('home.unpinned') : t('home.pinned'),
                body: isP
                  ? t('home.unpinnedBody')
                  : t('home.pinnedBody', { count: (usePinStore.getState().pins[convId]?.length ?? 0), max: PIN_MAX_PER_CONV }),
              })
              setCtxMenu(null)
            }}
            className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name={usePinStore.getState().isPinned(message.conversation_id, message.id) ? 'check' : 'pin'} size={13} />
            {usePinStore.getState().isPinned(message.conversation_id, message.id) ? t('home.unpinFromTop') : t('home.pinToTop')}
          </button>
          {/* 复制消息链接：hash 锚点格式 #msg={id}，贴到任意位置再点 → 切到该会话并滚动高亮 */}
          <button
            onClick={async () => {
              const link = `${window.location.origin}${window.location.pathname}#msg=${message.id}`
              try {
                await navigator.clipboard.writeText(link)
                useNotificationStore.getState().push({
                  tone: 'success',
                  title: t('home.linkCopied'),
                  body: t('home.linkCopiedBody'),
                })
              } catch {
                useNotificationStore.getState().push({
                  tone: 'error',
                  title: t('home.linkCopyFail'),
                  body: '',
                })
              }
              setCtxMenu(null)
            }}
            className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name="quote" size={13} />
            {t('home.copyMessageLink')}
          </button>
          {message.role === 'assistant' && onOpenVersions && versions.length > 0 && (
            <button
              onClick={() => {
                onOpenVersions(message, userMessageId)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="history" size={13} />
              {t('home.viewVersions')} ({versions.length})
            </button>
          )}
          {/* 1-5 星评分（纯前端 localStorage；与 like/dislike 互不干扰）—— 仅 assistant 消息 */}
          {message.role === 'assistant' && (
            <div className="border-t border-[var(--border)] mt-1 pt-1">
              <div className="px-3 py-1 text-[length:var(--app-text-2xs)] font-medium text-[var(--text-muted)] uppercase tracking-wider flex items-center justify-between">
                <span>{t('home.rateTitle')}</span>
                {useRatingStore.getState().get(message.id) && (
                  <button
                    onClick={() => {
                      useRatingStore.getState().remove(message.id)
                      useNotificationStore.getState().push({ tone: 'info', title: t('home.rateCleared'), body: '' })
                      setCtxMenu(null)
                    }}
                    className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] hover:text-[var(--danger)] normal-case tracking-normal"
                  >
                    {t('home.rateClear')}
                  </button>
                )}
              </div>
              <div className="flex items-center justify-around px-2 py-1.5">
                {[1, 2, 3, 4, 5].map((n) => {
                  const cur = useRatingStore.getState().get(message.id)
                  const active = cur?.score === n
                  return (
                    <button
                      key={n}
                      onClick={() => {
                        useRatingStore.getState().set(message.id, { score: n, comment: null })
                        useNotificationStore.getState().push({
                          tone: 'success',
                          title: t('home.rateSaved'),
                          body: t('home.rateSavedBody', { n }),
                        })
                        setCtxMenu(null)
                      }}
                      className={`w-7 h-7 rounded-md flex items-center justify-center text-[length:var(--app-text-base)] font-semibold transition-[color,background-color,border-color,box-shadow,transform] active:scale-90 ${
                        active
                          ? 'bg-[var(--warning)] text-white shadow-sm'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--warning)]'
                      }`}
                      title={t('home.rateStar', { n })}
                    >
                      {n}
                    </button>
                  )
                })}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
})


/* ============ 快捷键速查：键帽行（键 + 描述） ============ */
export function ShortcutRow({ keys, desc }: { keys: string[]; desc: string }) {
  return (
    <div className="flex items-center justify-between gap-2 py-1">
      <span className="text-[var(--text-secondary)] text-[length:var(--app-text-sm)]">{desc}</span>
      <span className="flex items-center gap-1 shrink-0">
        {keys.map((k, i) => (
          <span key={i} className="inline-flex items-center">
            <kbd className="px-1.5 py-0.5 text-[length:var(--app-text-xs)] font-mono font-medium rounded border border-[var(--border)] bg-[var(--bg-card)] text-[var(--text-primary)] min-w-[22px] text-center tnum">
              {k}
            </kbd>
            {i < keys.length - 1 && <span className="text-[var(--text-muted)] text-[length:var(--app-text-xs)] mx-0.5">+</span>}
          </span>
        ))}
      </span>
    </div>
  )
}
