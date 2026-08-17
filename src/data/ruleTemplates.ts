/**
 * 项目/全局指令模板库：覆盖常见开发场景的开箱即用规则。
 *
 * - 模板只是预设的 system prompt 文本片段，前端"应用模板"会写入对应 tab 的 textarea（不直接落库，需用户确认保存）
 * - 用户可自由修改、扩展、组合多个模板
 * - 新增模板只需在此数组加一条即可，i18n key 通过 `label` 引用
 */

export type RuleTemplateScope = 'global' | 'project' | 'both'

export interface RuleTemplate {
  /** 模板唯一 id，用于 React key + 命令面板引用 */
  id: string
  /** i18n key（label + description） */
  i18nKey: string
  /** 模板分类（用于下拉分组） */
  category: 'lang' | 'quality' | 'workflow' | 'docs' | 'meta'
  /** 适用作用域：global / project / 两者都可用 */
  scope: RuleTemplateScope
  /** 模板内容（写入 textarea 的文本） */
  content: string
}

/** 模板库：按 category 顺序排列，前端下拉按 category 分组 */
export const RULE_TEMPLATES: RuleTemplate[] = [
  // ============ lang: 语言/框架专用规则 ============
  {
    id: 'lang-typescript-strict',
    i18nKey: 'ruleTemplate.typescriptStrict',
    category: 'lang',
    scope: 'both',
    content: `## TypeScript 严格规范
- 启用 strict 模式（tsconfig.json: "strict": true）
- 禁止 any；不确定类型用 unknown + 类型守卫
- 函数返回值类型显式标注（公共 API 必须）
- 公共导出类型用 interface + 显式 export type
- 数组/Record 操作优先用不可变 API（map/filter/spread），禁止 push/splice 改原数组
- 错误处理用 unknown + 区分场景，never 兜底
- 命名：组件 PascalCase、变量 camelCase、常量 UPPER_SNAKE_CASE、布尔 is/has 前缀`,
  },
  {
    id: 'lang-rust-safe',
    i18nKey: 'ruleTemplate.rustSafe',
    category: 'lang',
    scope: 'both',
    content: `## Rust 安全规范
- 禁止 unwrap()，生产路径用 ? + 错误处理（thiserror/anyhow）
- 公开 API 返回 Result<T, E>，错误类型用 thiserror 派生
- 借用检查失败时优先重构数据流，不靠 unsafe 绕过
- 多线程共享数据优先 Arc<Mutex<T>> / Arc<RwLock<T>>；考虑 tokio::sync::Mutex 跨 await
- 字符串使用 &str / String 区分；Box<str> 用于不修改的堆分配
- 序列化/反序列化用 serde derive，禁止手写 impl Serialize
- 单元测试用 #[cfg(test)] 模块；集成测试放 tests/ 目录`,
  },
  {
    id: 'lang-arkts',
    i18nKey: 'ruleTemplate.arkts',
    category: 'lang',
    scope: 'both',
    content: `## ArkTS / HarmonyOS 规范
- 严格模式：禁止 any / Record<any, any> / JSON.parse 不带类型
- 接口定义优先用 interface，组件状态 @State 私有、@Prop 父传子、@Link 双向
- 列表渲染必须用 ForEach，禁止 map() 返回的 JSX 数组
- 资源引用用 $r('app.string.xxx')，禁止硬编码字符串
- 异步任务用 async/await + Promise；网络请求放 taskpool
- 路由跳转：router.pushUrl({ uri: 'pages/xxx' })，路由表必须在 main_pages.json 注册
- 日志用 hilog，禁止 console.log 提交到仓库`,
  },

  // ============ quality: 代码质量 ============
  {
    id: 'quality-no-debt',
    i18nKey: 'ruleTemplate.noDebt',
    category: 'quality',
    scope: 'global',
    content: `## 代码质量底线
- 禁止提交 TODO/FIXME/XXX 注释（未完成的直接写"未完成"中文 + 说明）
- 禁止死代码：未被引用的函数/常量/类型必须删除
- 禁止 console.log 调试日志留在生产代码
- 禁止魔法数字：常量提取到模块顶部 + 命名
- 禁止过深嵌套：函数内 if/for 嵌套不超过 3 层，超出则早返回
- 禁止超长函数：单个函数不超过 80 行（不含注释/空行）
- 重复代码 3 次以上必须抽取公共函数
- 公共 API 必须有中文 doc 注释（说明用途、参数、返回值、错误）`,
  },
  {
    id: 'quality-error-handling',
    i18nKey: 'ruleTemplate.errorHandling',
    category: 'quality',
    scope: 'global',
    content: `## 错误处理规范
- 预期错误：Result<T, E> / Promise.reject / 抛出业务异常
- 非预期错误：assert / panic / 中断流程
- 错误信息包含：上下文（哪个操作/哪个文件/哪个参数）+ 原因 + 建议
- 捕获后必须处理（log / rethrow / 兜底），禁止 catch 后空着
- 用户可见错误：友好提示 + 详细原因（hover/tooltip 展开）
- 不要用字符串匹配做错误分类，必须用错误码/类型`,
  },

  // ============ workflow: 工作流 ============
  {
    id: 'workflow-tdd',
    i18nKey: 'ruleTemplate.tdd',
    category: 'workflow',
    scope: 'project',
    content: `## TDD 流程
- 新功能先写失败测试 → 写最小实现 → 测试通过 → 重构
- 改 bug 先写能复现 bug 的测试 → 修复 → 测试通过
- 测试命名：describe('模块名') + it('场景_期望结果')
- 单元测试覆盖：正常路径 + 边界 + 异常路径
- 集成测试：跨模块的关键流程必须有端到端覆盖
- 提交前必须跑全部测试 + lint`,
  },
  {
    id: 'workflow-git-commit',
    i18nKey: 'ruleTemplate.gitCommit',
    category: 'workflow',
    scope: 'project',
    content: `## Git 提交规范
- Commit 信息格式：<type>(<scope>): <subject>
  - type: feat / fix / refactor / docs / style / test / chore
  - subject: 中文，不超过 50 字，不加句号
- Body 写"为什么改"而不是"改了什么"（diff 已经有了）
- 一个 commit 只做一件事；重构与功能改动分开
- 提交前 self-review：是否有调试代码 / 是否需要拆 commit
- 禁止提交：.env / node_modules / dist / target / .DS_Store`,
  },

  // ============ docs: 文档/注释 ============
  {
    id: 'docs-public-api',
    i18nKey: 'ruleTemplate.publicApi',
    category: 'docs',
    scope: 'project',
    content: `## 公共 API 文档规范
- 每个 export 的函数/类/接口必须有 JSDoc/doc comment
- 注释内容：用途 + 参数说明 + 返回值 + 抛出的错误 + 至少一个示例
- 复杂逻辑注释"为什么"而不是"做了什么"（what 看代码就行）
- 不确定的边界用 @todo / @deprecated 标记
- README 必须有：项目用途 + 快速开始 + 配置说明 + 常见问题`,
  },

  // ============ meta: 元规则（关于规则本身的规则） ============
  {
    id: 'meta-concise',
    i18nKey: 'ruleTemplate.concise',
    category: 'meta',
    scope: 'global',
    content: `## 沟通风格
- 回答直奔主题，避免冗长开场
- 复杂问题先列关键点（bullet）再展开
- 涉及代码改动时给：文件路径 + 关键 diff + 改的原因
- 不确定的方案给 2-3 个选项 + 我的推荐 + 理由
- 长任务先列实施步骤，确认后再动手`,
  },
  {
    id: 'meta-chinese',
    i18nKey: 'ruleTemplate.chinese',
    category: 'meta',
    scope: 'global',
    content: `## 中文优先
- 所有用户可见文案、注释、commit message、PR description 用中文
- 代码标识符（变量名/函数名/类名）保持英文
- 报错信息用户可见部分用中文
- 文档、README 用中文
- API key / 配置文件 key 保持英文`,
  },
]
