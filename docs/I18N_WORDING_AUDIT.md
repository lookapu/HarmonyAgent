# i18n 措辞一致性判读记录

> 本文件由 `scripts/audit_i18n_wording.py` 生成，**不要手改**。
> 重跑：`python scripts/audit_i18n_wording.py`

同一个中文句子对应多个 i18n 键时，中文侧相同是**正常的**
（导航项与页面标题本来就该用同一句话，本仓 120 组如此）。
要管的是英文侧：同一个词在同一屏界面里有两种写法。

## 判据

**不是「同一个中文就该同一个英文词」，而是
「两个键指的是不是同一界面里的同一个东西」。**

按钮 vs 状态、图例 vs 动作、面板 vs 表格列、集合 vs 单项——这些**本来就该不一样**。
中文过共享是**数据组织问题**，不是英文措辞问题；把两边硬拉平，
反而会在同一屏制造两个一模一样的标签去指两个不同的数。

实例：`cost.taskAvg` 与 `cost.toolAverageDuration` 中文都是「平均耗时」，
但前者取 `taskStats.avg_duration_ms`（任务级，同族 `taskLegend`/`taskCost`），
后者取 `reliability.tool_quality.average_duration_ms`（工具质量级，同族 `toolSuccessRate`）。
两个不同的数，英文的区分是对的。

## 汇总

| 档 | 组数 | 涉及键数 |
|---|---:|---:|
| 完全一致（正常） | 127 | 326 |
| **仅大小写不同** | 0 | 0 |
| 英文不一致 · **已书面解释** | 35 | 101 |
| 英文不一致 · **未解释** | 0 | 0 |
| 含空值跳过（归契约 4） | 0 | 0 |

**未解释 0 组**。门禁 `scripts/check-i18n-wording.py` 契约 5 会把「未解释」判为红，
契约 6 会把「解释表里有、但当前已无对应冲突组」的**失效条目**也判为红——
允许清单不报错的后果是组修好之后条目留在表里，
后来人以为「这组解释过了」，其实解释的是已经不存在的现象。

## 判读规则

| verdict | 含义 |
|---|---|
| `different-slot` | 同一个东西在不同槽位（动作/状态/图例/列头/导航 vs 页面标题），本来就该不同 |
| `different-thing` | 两个键指的根本不是同一个东西，中文侧过共享 |
| `zh-side` | 疑似中文侧标注问题，需产品口径，不擅自动用户可见文案 |
| `dead-key` | 该键在 src 里零引用，值已统一，键待清理 |

---

## 英文不一致的组（逐组附判读理由）

| 中文 | 键 | 英文 | verdict | 理由 |
|---|---|---|---|---|
| 平均耗时 | `cost.taskAvg`, `cost.toolAverageDuration` | Avg Duration / Average duration | different-thing | cost.taskAvg 在「任务」统计块，同族是 taskLegend/taskCost/taskTokens，取 taskStats.avg_duration_ms；cost.toolAverageDuration 在「工具质量」块，同族是 toolSuccessRate/toolArgumentErrors/toolTimeoutRate，取 reliability.tool_quality.average_duration_ms。两个不同的指标共用一句中文。 |
| 移除 | `dialog.remove`, `provider.syncRemove`, `apiKb.ctRemoved` | Remove / Remove / Removed | different-slot | dialog.remove / provider.syncRemove 是动作按钮（动词原形）；apiKb.ctRemoved 是状态标签「已移除」（过去分词）。 |
| 重新探测 | `health.sandboxProbe`, `health.envRedetect` | Probe again / Re-detect | different-slot | health.sandboxProbe 在 SandboxCapabilityPanel，是沙箱隔离能力的探测按钮；health.envRedetect 在 HealthPage 的环境区块。不同屏，用户不会同时看到。 |
| 添加项目 | `home.addProject`, `dialog.addProject`, `usage.event.project_added` | Add Project / Add Project / Project added | different-slot | home.addProject / dialog.addProject 是按钮；usage.event.project_added 是遥测事件名，出现在「使用统计」页的事件分布表里，用过去式。 |
| 依赖 | `home.analyzeDeps`, `home.cat_dependency` | Dependencies / Dependency | different-slot | home.analyzeDeps 是「分析依赖」的集合（复数）；home.cat_dependency 走模板字面量 t(`home.cat_${e.category}`)，是分类芯片（单数）。 |
| 模块 | `home.analyzeModuleList`, `apiKb.colModule` | Modules / Module | different-slot | home.analyzeModuleList 是集合复数；apiKb.colModule 是表格列头，列头习惯单数。 |
| 共 {{count}} 条 | `home.batchLines`, `home.importMsgCount`, `home.auditCount` | {{count}} lines / {{count}} messages / {{count}} entries | different-thing | home.importMsgCount 指导入的消息、home.auditCount 指审计条目，指的不是一类东西；home.batchLines 在英文侧只以 home.batchLines_other 存在，是 i18next 的复数约定（中文无复数变化），归键集对齐契约，不归措辞。 |
| 取消 | `home.cancel`, `dialog.cancel`, `trust.reject`, `provider.cancel`, `cost.cancelled`, `mcp.cancel`, `apiKb.cancel`, `common.cancel` | Cancel / Cancel / Cancel / Cancel / Cancelled / Cancel / Cancel / Cancel | different-slot | 七个 Cancel 是动作按钮；cost.cancelled='Cancelled' 是状态标签（过去分词）。 |
| 删除 | `home.deleteMessage`, `home.delete`, `home.diffRemoved`, `home.gitRemove`, `home.memoryDelete`, `home.previewDelete`, `home.deviceShotDelete`, `provider.delete`, `mcp.delete`, `knowledge.delete`, `apiKb.delete` | Delete / Delete / Removed / Remove / Delete / Delete / Delete / Delete / Delete / Delete / Delete | different-slot | 十个 Delete 是动作按钮；home.diffRemoved='Removed' 是 diff 图例；home.gitRemove='Remove' 是 Git 惯用词（git rm）。 |
| 启动 | `home.deviceAppLaunch`, `proxy.start` | Launch / Start | different-thing | home.deviceAppLaunch 启动的是设备上的 app；proxy.start 启动的是本地代理进程。宾语不同。 |
| 内存 | `home.devicePerfMem`, `home.detailRam` | Memory / RAM | different-thing | home.devicePerfMem 是百分比 PerfCard（unit="%"，内存占用率）；home.detailRam 是 GB 容量行（d.ram / 1024 / 1024）。两个不同的量，英文的区分是对的。 |
| 新增 | `home.diffAdded`, `home.memoryAdd`, `apiKb.ctAdded` | Added / Add / Added | different-slot | home.diffAdded / apiKb.ctAdded='Added' 是 diff 图例与状态标签（过去式）；home.memoryAdd='Add' 是动作按钮（动词原形）。 |
| 编辑消息 | `home.editMessage`, `usage.event.message_edited` | Edit message / Message edited | different-slot | home.editMessage 是动作按钮；usage.event.message_edited 是遥测事件名。 |
| 生成 | `home.generate`, `config.lanTokenCreateBtn` | Generate / Create | different-thing | home.generate 生成的是复现包 ZIP；config.lanTokenCreateBtn 创建的是局域网访问令牌。不同动作。 |
| 复制全文 | `home.importCopyAll`, `home.exportCopy` | Copy all / Copy full text | different-slot | home.importCopyAll 在导入弹窗的按钮上；home.exportCopy 在导出菜单项里。两个对话框不会同时呈现。 |
| 应用 | `home.modelRecApply`, `home.deviceTabApps` | Apply / Apps | different-thing | home.modelRecApply='Apply' 是「应用模型推荐」这个动作；home.deviceTabApps='Apps' 是设备面板的「应用」标签页。中文共用，英文完全不同的义。 |
| 已用 | `home.modelRecInUse`, `cost.balanceUsed` | In use / Used | different-slot | home.modelRecInUse 是模型推荐行的徽章（chat/components/plan.tsx）；cost.balanceUsed 在费用页的余额区。不同屏。 |
| 项目路径 | `home.projectPath`, `trust.path` | Path / Project path | different-slot | home.projectPath 在项目概览行里，与同族 home.projectName='Name' 构成短式对齐（Name / Path）；trust.path 在信任对话框里独立成行，用完整写法更清楚。 |
| 沟通风格 | `home.ruleTemplateCategory.meta`, `home.ruleTemplate.concise` | Communication / Concise Communication | different-thing | home.ruleTemplateCategory.meta='Communication' 是分类名；home.ruleTemplate.concise='Concise Communication' 是该分类下具体模板的名字。 |
| 继续任务 | `home.slashContinue`, `home.continueTask` | Continue task / Continue | different-slot | home.slashContinue 是斜杠命令项——它脱离上下文出现在命令面板里，必须自解释，所以更完整；home.continueTask 是有上下文的按钮。 |
| 系统 | `home.timelineEventSystem`, `home.detailOs`, `health.jdkSrcSystem` | System / OS / System | different-slot | home.detailOs='OS' 是设备详情行，OS 比 System 更精确；home.timelineEventSystem 是时间线事件标签，health.jdkSrcSystem 是 JDK 来源选项。 |
| 工具调用 | `home.timelineEventToolCall`, `home.toolCallsShort`, `license.quotaToolCalls` | Tool / Tool calls / Tool calls | different-slot | home.timelineEventToolCall='Tool' 是时间线条目的短标签（宽度受限）；home.toolCallsShort 与 license.quotaToolCalls 是统计与配额标签。 |
| 取消归档 | `home.unarchive`, `home.auditLabelConvUnarchive` | Unarchive / Unarchive conversation | different-slot | home.unarchive 是会话右键菜单项（短式）；home.auditLabelConvUnarchive 是审计筛选标签，第十八批统一成带宾语，与同族 Delete conversation / Pin conversation 对齐。 |
| 取消置顶 | `home.unpinProject`, `home.unpin`, `home.unpinFromTop`, `home.auditLabelConvUnpin` | Unpin / Unpin / Unpin from top / Unpin conversation | different-slot | home.unpin / home.unpinProject 是菜单项（短式）；home.auditLabelConvUnpin 是审计筛选标签（带宾语）；home.unpinFromTop 是消息级置顶，用 Unpin from top 与项目级/会话级区分。第十八批已判定「Pin to top / Unpin from top」这一对内部对称，不动。 |
| 内置 | `knowledge.builtin`, `health.installNode` | Built-in / Install bundled | different-thing | knowledge.builtin='Built-in' 是属性「内置的」；health.installNode='Install bundled' 是动作「安装内置版本」。 |
| 分钟 | `limits.unitMin`, `cost.minutes` | min /  min | different-slot | limits.unitMin='min' 是独立的单位标签；cost.minutes=' min' 的**前导空格是功能性的**——它被 formatDuration(ms, t('cost.minutes')) 当作单位后缀拼接，去掉空格会渲染成「12min」。这一条不是措辞差异，是数据里的功能性空白。 |
| 禁用 | `mcp.disable`, `skill.disable`, `skill.disabled` | Disable / Disable / Disabled | different-slot | mcp.disable / skill.disable 是动作按钮；skill.disabled 是状态标签。 |
| 启用 | `mcp.enable`, `skill.enable`, `skill.enabled`, `knowledge.enabled` | Enable / Enable / Enabled / Enabled | different-slot | mcp.enable / skill.enable 是动作按钮；skill.enabled / knowledge.enabled 是状态标签。 |
| 推荐 | `mcp.popular`, `skill.popular`, `health.jdkRecommended` | Popular / Popular / Recommended | different-thing | mcp.popular / skill.popular='Popular' 指社区流行度；health.jdkRecommended='Recommended' 指官方推荐的 JDK 版本。中文共用，英文是两种含义。 |
| 问题复现包 | `nav.reproductionBundles`, `repro.title` | Reproduction Bundles / Issue Reproduction Bundles | different-slot | nav.reproductionBundles 是侧栏导航（短），repro.title 是页面标题。全仓既定模式就是「nav 短、页面标题长」：Cost / Cost Tracking、MCP / MCP Servers、Health / Health Monitor、Skill / Skill Management、Version / Version Management 同款。 |
| 使用统计 | `nav.usage`, `config.telemetry`, `usage.title`, `mcp.usageView`, `skill.usageView` | Usage / Usage stats / Usage Stats / Usage / Usage | different-slot | 三个域各按自己的规则，全对：nav.usage / mcp.usageView / skill.usageView 是导航与视图标签（短式）；config.telemetry 是设置项行标签（句首式 'Usage stats'）；usage.title 是页面标题（Title Case 'Usage Stats'）。第十六批定的规则明确「面板标题确实是 Title Case」。 |
| 添加工程 | `onboarding.step.project`, `onboarding.projectAdd` | Add a project / Add project | different-slot | onboarding.step.project='Add a project' 是引导步骤标题，与同族 onboarding.step.env='Check your environment'、onboarding.step.provider='Connect a model' 同为「动词 + 冠词 + 名词」；onboarding.projectAdd 是按钮。 |
| 模型 | `provider.models`, `cost.evalModel`, `cost.model` | Models / Model / Model | different-slot | provider.models='Models' 是模型列表的**分区标签**（ProvidersPage 里两个 <label>，指一整片列表）；cost.model='Model' 是**表格列头**（4 处 <th>，指某一行的那一个模型）；cost.evalModel='Model' 是行内指标（`{model} · {usage}`）。集合标签用复数、列头用单数是既定做法，与「技能」组（skill.skillsView='Skills' / skillStats.colSkill='Skill'）同形状。本组是第十九批修 provider.models 的中文标注时**才暴露**的——它原先被错标成「默认模型」，与 provider.modelDefault 挤在同一组，把这一组漂移整个盖住了。**中文标错会让另一组漂移一直看不见。** |
| 技能 | `skill.skillsView`, `skillStats.colSkill` | Skills / Skill | different-slot | skill.skillsView='Skills' 是视图标签（复数）；skillStats.colSkill='Skill' 是表格列头（单数）。 |
| 撤销 | `teamShare.revert`, `config.lanTokenRevoke` | Revert / Revoke | different-thing | teamShare.revert='Revert' 还原的是代码改动；config.lanTokenRevoke='Revoke' 吊销的是访问令牌。不同动作。 |

---

## 修正记录（第十八批）

共改 36 键，全部**按 key 定点替换**（顶层段 + 叶键 + 旧值三重锚定，段内唯一），
不是 `json.load` + `json.dump` 全量重写——后者会重排整个文件、产生几百行假 diff。

| 段 | 键数 | 内容 |
|---|---:|---|
| A 真漂移 | 6 | 确认删除？/ 已删除 / 保存失败 / 成功率 / 调用次数 / 引用文件 |
| B chat -> conversation | 7 | 「会话记录」这一个名词义项；全库 50 : 22 |
| C 审计族内部一致 | 3 | `home.auditLabel*` 三个不带宾语的补上宾语 |
| D 省略号 | 13 | 忙碌态语义簇的 `...` -> `…`（U+2026） |
| E 第二轮 | 7 | 第一轮漏判的组：当前 / 切换模型 / 刷新 / 重试 / 读取失败 |

### B 段的决定性证据

同一句话里就混着两个词：
`home.noConversation` = "No **conversations** yet. Click New **Chat** to start"，
侧栏标题是 `home.conversations` = "Conversations"，
而同一个右键菜单里是 "Pin **chat** / Archive **chat** / Delete **chat**"。

但 22 个含 chat 的键里**有一大半不是漂移**，一律不动：
`Send to chat`（动词短语）、`Chat settings`、`in the chat area`、
`neither chat nor tools` —— 那些是「对话区域 / 对话行为」义项，
改成 conversation 语法就错了（"Send to conversation"）。

### 三条被证据推翻、因此不改

- `repro.title`「Issue Reproduction Bundles」**不该**缩短：
  全仓页面标题域的既定模式就是「nav 短、页面标题长」——
  Cost / Cost Tracking、MCP / MCP Servers、Health / Health Monitor 同款。
- `config.telemetry`「Usage stats」**不该**改成 Title Case：
  它是设置项行标签（句首式），`usage.title` 是页面标题（Title Case），
  两者本来就该不同；改了就与第十六批刚统一的规则相反。
- `home.detailRam`「RAM」**不该**改成「Memory」：
  `home.devicePerfMem` 是**百分比** PerfCard（`unit="%"`），
  `home.detailRam` 是 **GB 容量**行。两个不同的量。

---

## 完全一致的一档（抽样 15 组，说明这一档不是问题）

| 中文 | 键 | 英文 |
|---|---|---|
| 局域网访问 | `nav.lan`, `config.lanSection` | LAN Access |
| Provider | `nav.provider`, `cost.autoPoolProvider` | Provider |
| 版本 | `nav.version`, `about.versionTitle`, `apiKb.colVersion` | Version |
| 工具限制 | `nav.limits`, `limits.title` | Tool Limits |
| 费用 | `nav.cost`, `cost.fee` | Cost |
| 授权与权益 | `nav.license`, `license.title` | License & Entitlements |
| 团队共享 | `nav.teamSharing`, `teamShare.title` | Team Sharing |
| 关于 | `nav.about`, `about.title` | About |
| 请先在工作区选择项目 | `teamShare.selectProject`, `repro.selectProject` | Select a project in the workspace first |
| 重试 | `teamShare.retry`, `repro.retry`, `home.retry`, `home.timelineRetry`, `license.retry`, `common.retry` | Retry |
| 预览内容 | `repro.preview`, `config.diagPreview` | Preview contents |
| 复制 | `md.copy`, `home.selectionCopy`, `home.errorCopy`, `config.lanTokenCopy` | Copy |
| 会话 | `home.conversations`, `home.searchModeConv`, `config.scaleConv` | Conversations |
| 置顶会话 | `home.pin`, `home.auditLabelConvPin` | Pin conversation |
| 归档会话 | `home.archive`, `home.auditLabelConvArchive` | Archive conversation |
