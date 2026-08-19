/**
 * 前端共享常量：集中所有 localStorage 存储键，消除硬编码字符串与重复定义。
 *
 * 约定：
 * - 键的字符串值刻意保持历史原样，以兼容用户已写入的 localStorage 数据（切勿随意改名）。
 * - 带 `PREFIX` 后缀的键用于拼接（如 `STORAGE_KEYS.LAST_CONV_PREFIX + projectId`）。
 * - 读取 / 写入请统一经 src/utils/storage.ts 的封装（容错 + JSON 辅助）。
 */

export const STORAGE_KEYS = {
  // 国际化
  LANG: 'deveco-switch-lang',

  // 主题
  THEME: 'deveco-switch-theme',
  THEME_LAST: 'deveco-switch-theme-last',

  // 项目 / 会话记忆
  LAST_PROJECT: 'deveco-switch:last-project-id',
  LAST_CONV_PREFIX: 'deveco-switch:last-conv:',
  GIT_REPO_PREFIX: 'deveco-switch:git-repo:',

  // 会话维度 store
  CONV_NOTES: 'deveco-switch-conv-notes',
  AUDIT_LOG: 'deveco-switch-audit-log',
  PINNED: 'deveco-switch-pinned-messages',
  RATINGS: 'deveco-switch-message-ratings',

  // 页面 / 设置
  VERSION_PROXY: 'deveco-switch-version-proxy',
  PERF_MONITOR: 'deveco-switch:perf-monitor',
  THINKING_OPEN: 'deveco-thinking-open',
  TOOL_OPEN_PREFIX: 'deveco-tool-open-',
  BALANCE_ALERTED: 'deveco-balance-alerted',
  TOOLCHAIN_PATHS: 'deveco-switch-toolchain-paths',

  // Home 工作区布局 / 偏好
  PREVIEW_URL: 'deveco-switch-preview-url',
  CHAT_OPTIONS: 'deveco-switch-chat-options',
  DRAFTS_PREFIX: 'deveco-switch-drafts:',
  RIGHT_PANEL: 'deveco-switch-right-panel',
  SIDEBAR_COLLAPSED: 'deveco-switch-sidebar-collapsed',
  SIDEBAR_WIDTH: 'deveco-switch-sidebar-width',
  RIGHT_WIDTH: 'deveco-switch-right-width',

  // Home 跨会话 UI 状态
  SCROLL_POS: 'deveco-scroll-pos',
  UNREAD_MAP: 'deveco-unread-map',
  REF_MRU: 'deveco-ref-mru',
} as const

export type StorageKey = (typeof STORAGE_KEYS)[keyof typeof STORAGE_KEYS]
