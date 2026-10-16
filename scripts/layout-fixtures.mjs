/**
 * 布局探针的 fixture —— 一份确定性的假 Tauri 后端
 *
 * ## 为什么需要它
 *
 * 应用所有数据都走 `window.__TAURI_INTERNALS__.invoke`，浏览器里没有这层就只会渲染
 * 错误页（实测：全部返回 null 时 Home 崩在 `.length`，ErrorBoundary 接住）。
 * 所以要让浏览器渲染出**真实布局**，必须提供形状正确的假数据。
 *
 * ## 为什么不放截图基线
 *
 * 字体渲染、抗锯齿、GPU 光栅在不同机器上不一致，截图 diff 的阈值只能调得很宽，
 * 宽到「真回归也看不出」；而二进制基线进 git 会让每次改动都是一堆不可读的 diff。
 * **P1-4 的验收判据「1100×720 多显示约 15% 内容」本身就是一个可计数��**，
 * 直接量比像素比对更准也更稳：结论是「多显示 3 行会话」而不是「17.3% 的像素变了」。
 *
 * ## 数据为什么要「长得像真的」
 *
 * fixture 的行高会直接影响测量值。用 1 条会话去测「能显示几行」没有意义，
 * 也不像真实场景。这里给 40 条会话 + 30 条消息，长度按中英混排取。
 * **fixture 一旦改动，所有基线数字都要重测**——`check-density.py` 会因此报红，
 * 这是刻意的。
 */

/** 确定性伪随机：同一个 seed 永远同一串，用来生成不重复但可复现的名字 */
function seeded(seed) {
  let s = seed >>> 0
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0
    return s / 0x100000000
  }
}

const WORDS = [
  '重构会话列表的虚拟滚动', '修复中文字体回退', '补 Harmony 子工程索引',
  '压缩 Home 初始 chunk', '统一空态组件', '接入 tree-sitter 符号索引',
  'Agent 审批流水线', '重试策略与退避', '构建产物体积门禁', '快捷键冲突排查',
  'SQLite 迁移回滚', '成本页按日聚合', 'Provider 超时处理', '多 Provider 轮询',
  'LAN 令牌轮换', '诊断包脱敏', '首启引导三步', '设置页三段式', '主题切换闪白',
  '长会话滚动性能', 'MCP 工具白名单', '技能包签名校验', '日志滚动清理',
  '工作区模块识别', '子工程相对路径', '构建流水线缓存', '沙箱能力探测',
  '依赖许可核对', '视觉回归基线', '密度与排版收敛', '字号阶梯接通',
  'a11y 语义补齐', '动效降级策略', '隐私承诺文案', '导出 NOTICE',
  '本地使用统计', '埋点事件名对齐', '剪贴板图片粘贴', '终端分屏布局',
]

const MODELS = ['gpt-4o', 'claude-sonnet-4', 'deepseek-v3', 'qwen-max']
const PROVIDERS = ['OpenAI', 'Anthropic', 'DeepSeek', 'DashScope']

/**
 * 冻结的「现在」（秒）。
 *
 * 会话行上有三处逻辑依赖**当前时间**：时间分组（今天/昨天/本周/更早）、
 * 归档建议徽章（7 天 / 30 天两档）、相对时间显示。这三处一旦随真实日期漂移，
 * 基线就会在某一天自己变掉——而没人知道是哪天、为什么。
 *
 * 所以 shim 在导航前把 `Date` 钉死在这个瞬间（见 layout-probe-shim.js），
 * fixture 的所有时间戳都相对它生成。**这个常量是唯一真源，两处不许各写一个数字。**
 */
export const FROZEN_NOW = 1767225600 // 2026-01-01T00:00:00Z

/** 天 → 秒。写成一个具名函数而不是内联 `* 86400`，好让 check-density.py 能解析出「天」数 */
const d = (n) => n * 86400

/**
 * 40 个会话各自的「距今多久」（秒，升序 → 列表里最新在最前）。
 *
 * 这不是随手填的数组，它要同时踩到：
 *   - **四个时间分组**：今天 / 昨天 / 本周 / 更早
 *   - **两档归档徽章**：7 天 →「建议归档」、30 天 →「建议清理」
 *   - **置顶豁免**：`suggestArchive` 对 `is_pinned` 的会话直接返回 normal
 *
 * 早期版本 40 个会话全是「273 天前」，于是每行都挂上「建议清理」徽章，
 * 元信息行被挤到**两行**（11px 字号 × 1.5 行高 × 2），行高从 50px 涨到 66.5px。
 * 数字「看着稳定」，量的却是**最坏态而不是常态**——基线因此失去意义。
 * 这类错误 grep 不出来、截图也看不出来，只有把行高拆开量才会暴露。
 *
 * 数值必须写死，不许「按第 i 个会话推算」：任何跨过 7 天 / 30 天阈值的算法，
 * 都会在真实日期推进时悄悄换掉徽章状态，而基线要的是**可复现**而不是「平均」。
 */
const CONV_AGES = [
  // 今天（6）：0.5h ~ 12h
  1800, 5400, 9000, 18000, 28800, 43200,
  // 昨天（6）：1.04d ~ 2.7d
  90000, 108000, 126000, 162000, 198000, 234000,
  // 本周（8）：3d ~ 6.5d，全部 < 7 天阈值 → 不挂徽章
  d(3), d(3.5), d(4), d(4.5), d(5), d(5.5), d(6), d(6.5),
  // 更早·弱提醒（8）：10d ~ 28d，7~30 天之间 →「建议归档」
  d(10), d(12), d(15), d(18), d(20), d(22), d(25), d(28),
  // 更早·强提醒（12）：45d ~ 420d，> 30 天 →「建议清理」
  d(45), d(60), d(75), d(90), d(120), d(150), d(180), d(210), d(240), d(300), d(360), d(420),
]

/**
 * 置顶的会话下标。
 *
 * 0 是今天那档（本来就���徽章），20 与 21 落在 7 天以上那档：
 * 挑它们是为了让「置顶豁免归档徽章」这条逻辑在基线里**真的被跑到**——
 * 否则 pin 住的全是本来就没徽章的行，这条短路等于没测。
 * check-density.py 会核对「至少有一个置顶项落在 7 天以上」，下标改错会报红。
 */
const PINNED_IDX = new Set([0, 20, 21])

function conversations(n) {
  const rnd = seeded(20260814)
  return Array.from({ length: n }, (_, i) => {
    const w = WORDS[Math.floor(rnd() * WORDS.length)]
    const age = CONV_AGES[i % CONV_AGES.length]
    return {
      id: `conv-${i}`,
      title: i === 0 ? w : `${w}（${i + 1}）`,
      project_id: 'proj-1',
      provider_id: null,
      model_id: null,
      system_prompt_version: null,
      // 字段名必须与 src/api/project.ts 的 Conversation **逐字一致**：
      // 写成 `pinned` / `message_count` 时 TypeScript 不会报错（fixture 是 .mjs，
      // 不参与类型检查），但 UI 读到的是 undefined——**形状错了但编译是绿的**。
      // check-density.py 会对账这条契约。
      is_pinned: PINNED_IDX.has(i),
      archived: false,
      tags: '',
      work_mode: 'local',
      worktree_path: null,
      worktree_branch: null,
      messages_count: 2 + Math.floor(rnd() * 40),
      created_at: FROZEN_NOW - age - 3600,
      updated_at: FROZEN_NOW - age,
    }
  })
}

function messages(n) {
  const rnd = seeded(987654321)
  return Array.from({ length: n }, (_, i) => ({
    id: `msg-${i}`,
    conversation_id: 'conv-0',
    role: i % 3 === 0 ? 'user' : 'assistant',
    content:
      i % 3 === 0
        ? '这个模块的布局在 1100×720 下太挤了，能不能再紧凑一档？'
        : // 文案要**描述真正做过的改动**。原先这里写的是「行高从 1.6 降到 1.45」，
          // 而实际收敛的是内边距与元信息行距——fixture 里的假对话描述了一件没发生的事，
          // 后来者照着它排查「行高为什么没变」纯属浪费。
          // 文案长度参与布局，改动后必须重跑 layout-probe --update。
          '已经收敛一档：会话行内边距 12px 降到 8px，元信息行距 2px 降到 1px。默认窗口下从 6 行变成 7 行。',
    // 相对 FROZEN_NOW 递增：列表按时间升序渲染，最后一条是最新的
    created_at: FROZEN_NOW - (n - i) * 600,
    tokens: 120 + Math.floor(rnd() * 800),
  }))
}

export const FIXTURES = {
  list_projects: [
    {
      id: 'proj-1',
      name: 'HarmonyAgent',
      path: 'C:/work/HarmonyAgent',
      kind: 'harmony',
      trusted: true,
      default_provider_id: 'p-1',
      default_model_id: 'm-1',
      index_state: 'ready',
      rules: null,
      last_opened_at: FROZEN_NOW,
      created_at: 1755000000,
      worktree_path: null,
      harmony_subprojects: JSON.stringify(['entry/src/main/ets', 'feature/auth']),
      workspace_modules: JSON.stringify([
        { name: 'entry', kind: 'entry', path: 'entry' },
        { name: 'feature-auth', kind: 'feature', path: 'feature/auth' },
      ]),
      harmony_project_path: null,
      conversation_count: 40,
      pinned: true,
    },
  ],
  list_providers: PROVIDERS.map((name, i) => ({
    id: `p-${i + 1}`,
    name,
    kind: 'openai-compatible',
    base_url: `https://api.example.com/${i + 1}`,
    models: [
      { id: `m-${i + 1}a`, name: MODELS[i] },
      { id: `m-${i + 1}b`, name: MODELS[(i + 1) % MODELS.length] },
    ],
    active: i === 0,
    has_key: true,
    enabled: true,
  })),
  list_tool_groups: [
    ['build', '构建'],
    ['fix', '修复'],
    ['explore', '探索'],
    ['test', '测试'],
    ['other', '其他'],
  ],
  tools_health: [
    { name: 'hvigorw', found: true, detail: 'C:/Huawei/hvigorw/bin/hvigorw.bat', suggestion: null },
    { name: 'hdc', found: true, detail: 'C:/Huawei/sdk/toolchains/hdc.exe', suggestion: null },
    { name: 'ohpm', found: true, detail: 'C:/Huawei/ohpm/ohpm.bat', suggestion: null },
  ],
  list_palette_commands: [
    { id: 'nav:/providers', title: '供应商', subtitle: '打开供应商页', group: '导航', icon: 'bolt' },
    { id: 'nav:/config', title: '设置', subtitle: '打开设置页', group: '导航', icon: 'settings' },
    { id: 'nav:/cost', title: '成本', subtitle: '打开成本页', group: '导航', icon: 'payments' },
    { id: 'action:newConversation', title: '新建会话', subtitle: '', group: '动作', icon: 'chat' },
  ],
  project_scoped_counts: { 'proj-1': { mcp: 3, skills: 5 } },
  check_base_update: { current: '2.3.0', latest: '2.3.0', can_update: false, package: 'stable' },
  get_system_proxy: null,
  get_harmony_env: {
    configured: true,
    hvigor_path: 'C:/Huawei/hvigorw/bin/hvigorw.bat',
    hdc_path: 'C:/Huawei/sdk/toolchains/hdc.exe',
    ohpm_path: 'C:/Huawei/ohpm/ohpm.bat',
    devEco_path: 'C:/Program Files/Huawei/DevEco Studio',
    node_path: 'C:/Program Files/nodejs/node.exe',
  },
  get_settings: {},
  list_conversations: conversations(40),
  list_messages: messages(30),

  // ---- 以下为补齐「首屏实际会调的命令」时逐个加的 ----
  // 消息列表真实走的是 list_messages_page（分页），不是 list_messages。
  // 给它 30 条才能量出「一屏能放下几条消息」——1 条的话任何密度改动都量不出来。
  //
  // **必须是 MessagePage 对象而不是裸数组**（{ messages, hasMore }）：
  // chatSlice 直接写 `messages: page.messages`，形状给错时 store 里的
  // messages 变成 undefined，Home 随后在 `messages.length` 上崩、整页进
  // ErrorBoundary——而报错里完全看不出根因是 fixture 形状错了。
  list_messages_page: { messages: messages(30), hasMore: false },
  list_conversation_tags: ['bug', 'feature', 'refactor', 'docs'],
  get_task_runs: [],
  list_pending_confirmations: [],
  list_queued_messages: [],
  list_memories: [],
  list_tool_stats: [],
  list_project_dir: ['entry', 'feature', 'oh_modules', 'build-profile.json5'],
  list_provider_models: MODELS.map((m) => ({ id: m, name: m, provider_id: 'p-1' })),
  get_harmony_root: 'C:/Huawei',
  get_git_branches: ['main', 'dev', 'feature/pinyin-lazy'],
  git_branch_info: { current: 'main', upstream: 'origin/main', ahead: 0, behind: 0 },
  get_conversation_branch_parent: null,
  conversation_context: { files: [], symbols: [], tokens_used: 0, token_budget: 200000 },
  // 形状取自 src/api/project.ts 的 ConversationContextV2。
  // 少给字段的后果是 Home 在 `ctxV2Detail.pins.filter` 上崩——
  // 而报错里完全看不出「是 fixture 少了 pins 字段」。
  get_conversation_context_v2: {
    schema_version: 2,
    conversation_id: 'conv-0',
    summary: null,
    summary_from_message_rowid: 0,
    summary_to_message_rowid: 0,
    summary_event_seq: 0,
    task: { run_id: null, goal: '', state: 'idle', phase: '', required_conditions: [], constraints: [], completed_steps: [] },
    hot: { facts: [] },
    artifacts: [],
    pins: [],
    budget: { tokens_used: 0, token_budget: 200000, budget_usage_ratio: 0, summary_coverage: 0, summary_depth: 0, recent_invalidations: [], degraded: false, advice: [] },
    facts_digest: null,
    invalidation_epoch: 0,
    reconciliation: { updated_at: 0 },
  },
  get_session_health: {
    budget_usage_ratio: 0, summary_coverage: 0, summary_depth: 0,
    recent_invalidations: [], degraded: false, advice: [],
  },
  warmup_symbol_index: null,

  // 下面几个是「返回 null 会让调用方 .length 崩」的那批，
  // 形状取自 src/api/project.ts 的类型标注，不是猜的
  get_latest_agent_run: null,           // AgentRun | null
  list_message_feedback: [],            // MessageFeedback[]
  list_message_versions: [],            // MessageVersion[]
  get_todos: [],                        // TodoItem[]
  get_task_ledger: null,                // TaskLedger | null
  get_ask: null,                        // PendingAsk | null
  build_project_index: {                // FileTreeNode
    name: 'HarmonyAgent', path: 'C:/work/HarmonyAgent', kind: 'dir',
    children: [
      { name: 'entry', path: 'C:/work/HarmonyAgent/entry', kind: 'dir', children: [] },
      { name: 'feature', path: 'C:/work/HarmonyAgent/feature', kind: 'dir', children: [] },
    ],
  },
  conversation_cost_stats: {            // ConversationCostStats
    total_cost: 0, total_tokens_in: 0, total_tokens_out: 0, request_count: 0, by_day: [],
  },
}

/**
 * Tauri 插件命令的通用返回。
 *
 * 与应用数据分开：这些是**基础设施**（事件订阅、窗口主题、文件监听、代理），
 * 返回值形态由插件约定而非本项目决定，逐一造 fixture 只会让清单变长且没人看。
 * 但**不能一律返回 null**——`plugin:event|listen` 要返回事件 id，
 * `plugin:updater|check` 要返回带 can_update 的对象，否则对应调用点会崩。
 */
export const PLUGIN_DEFAULTS = {
  'plugin:event|listen': 1,
  'plugin:event|unlisten': null,
  'plugin:window|set_theme': null,
  // **必须是 null，不能是 `{ shouldUpdate: false, manifest: null }`。**
  // @tauri-apps/plugin-updater 的 check() 是 `return metadata ? new Update(metadata) : null`
  // —— 它**只看 metadata 真假，根本不读 shouldUpdate**。于是写成
  // `{ shouldUpdate: false }` 照样造出一个真值 Update，右下角弹出「发现新版本」浮层。
  // Rust 侧返回的是 Option<UpdateMetadata>，无更新就是 None → null。
  // 顺带一提：这个浮层是 `fixed bottom-4 right-4`，**不覆盖**会话列表与消息区，
  // 所以它不影响本批任何测量值；改成 null 是为了「量常态布局」这条原则成立，
  // 以及不让截图看起来像首启那一瞬。
  'plugin:updater|check': null,
  'plugin:fs|watch': 1,
  'plugin:fs|unwatch': null,
  'begin_update_proxy': null,
  'end_update_proxy': null,
  'plugin:resources|close': null,
}
