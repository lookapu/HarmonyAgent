# DevEco Switch · CHANGELOG

> 面向 HarmonyOS / OpenHarmony 开发者的桌面 AI 编程 IDE。
> 本文件按版本倒序记录用户可见的变更、迁移要点与回滚指引。

---

## Unreleased — 证据驱动治理与双层执行内核（2026-08-21）

定位：把“模型能调用很多工具”升级为“任务和工具都可持久调度、可验收、可恢复、可观测”。可靠性与治理批次新增迁移 `057`—`062`、`069`—`075`；当前继续推进长会话 Context V2，数据库迁移总数达到 **75**。治理批次共新增 3 个对外工具（`workflow_template`、`team_share`、`reproduction_bundle`），`TOOL_SPECS` 达到 **201**。

- 固定评测新增 schema v1 执行快照：记录真实模型/提示词使用状态、工具注册表摘要、去路径化 SDK 版本、哈希设备身份、Token、成本、总耗时和逐用例/最终证据摘要；历史运行按 schema 0 兼容读取。
- CI 新增评测基线回退门禁：保存/恢复可跨机器比较的基线，阻止任务完成率、评测覆盖或关键延迟显著回退；重复副作用率与恢复率继续由崩溃恢复 E2E 硬门禁裁决，主分支保存基线、PR 只比较。
- 新增失败样本回流工具：校验复现包完整性/脱敏状态并提炼评测场景草案，真实失败可转化为固定评测回归场景并纳入 CI 覆盖。
- 新增统一资产版本清单：数据库、工具协议、Skill/工作流规范、知识索引与评测 schema 的当前版本、兼容承诺和迁移说明集中可查，发布说明与验证共用同一数据源。
- 发布说明自动汇总：从 git 对比与 CHANGELOG 生成迁移清单、工具协议变更、资产版本、风险与回滚方式，release.yml 发布流程自动使用。
- 新增文档漂移门禁 `scripts/check-docs.py`：从代码真源提取工具/迁移/IPC/模块数量并与 README、架构文档逐模式比对，校验路线图与 docs 内链接、代码路径引用及 CI 工作流引用的测试和脚本；接入 quality.yml 双平台门禁，文档与实现脱节时合并被阻断。
- 告警收敛与不新增基线门禁（Q-07）：ESLint 9 项告警清零（react-hooks 真修复与带原因豁免，`--max-warnings 0` 阻断回退）；Clippy 338 → 44（clippy --fix 两轮收敛 221 项机械告警，批量 sort_by_key 转换，修复锁文件截断语义、Drop 中进程终止等疑似真 bug，文档格式批量规范化），剩余 44 项全为结构类（too_many_arguments 31 + type_complexity 13）保留为基线；`scripts/check-warnings.py` 按 (lint, 位置) 去重统计 clippy 唯一告警并接入 quality.yml，新增告警立即阻断。

- 建立 `agent_harmony_fixed_v3` 固定评测集：16 个可靠性场景与 10 个鸿蒙场景统一运行，覆盖真实工程创建内核、编译 API 归因、跨模块影响、录制真机 faultlog、混合工程和长会话恢复。
- 新增可解释的鸿蒙指纹报告，把工程清单、ArkTS/ArkUI、`@kit.*` / `@ohos.*` 和构建/崩溃日志证据接入 `get_project_info` 与能力包选择，同时保留普通 TypeScript 负例和“不得由导入风格猜测精确 API Level”的边界。

- 新增问题复现包页面和 `reproduction_bundle` 工具，将问题描述、工程语义环境、可选会话/工具/Run 证据及项目内文本附件导出为本地 ZIP，不自动上传或分享。
- 生成前展示逐项路径、大小、脱敏状态、遗漏原因与 SHA-256；确认时重新采集并绑定预览摘要，内容变化则要求重新预览，Agent 每次生成需新鲜显式审批。
- ZIP 使用项目边界检查、不可覆盖临时文件和 manifest 逐项摘要，自校验后原子提交；生成历史保存整个文件摘要，可再次检测篡改、截断或条目缺失。
- 复现包复用统一字段/文本脱敏并额外遮罩工程根与用户目录；二进制、越界路径、凭据、证书、keystore 和签名材料默认拒绝。结构化应用日志也改为脱敏后以私有权限落盘。

- 新增 schema v1 团队共享包和管理页，可导入/导出项目记忆、工程约定与固定评测集；包绑定来源 URI、精确修订、SemVer 和规范化摘要，同版本内容不可漂移。
- 导入前逐项预览新增、同源更新、本地冲突和未变化项；冲突只生成禁用且未确认的团队副本，不覆盖本地事实，Agent 应用与撤销每次要求显式审批。
- 导入批次与逐项变更持久留痕，更新保存恢复快照；撤销会先复验来源、稳定 key 和导入后摘要，用户接管或修改过的内容安全保留。
- 团队评测集只能组合本机已注册场景并锁定 expected 契约，拒绝任意代码与未知场景；应用/撤销进入统一审计。

- 第三方 Skill、MCP 与工作流模板共用扩展治理账本：记录来源、精确修订和 SHA-256，支持 Ed25519 分离签名，并明确区分签名有效与发布者身份可信。
- 扩展调用新增每实例分钟配额和跨重启持久熔断；Skill 内容漂移、无效签名和熔断实例失败关闭，不影响其它扩展。
- 扩展登记、验签、调用、限流、失败和策略变化进入统一审计；Skill/MCP 页面展示真实性和隔离状态，治理 IPC 支持只读盘点和受限策略调整。

### HarmonyOS 工程语义模型

- 新增版本化 `HarmonySemanticModel` 单一解析真源，统一表示应用、产品、嵌套模块、HAP/HSP/HAR 产物类型、Ability、ExtensionAbility 和 OHPM 依赖边。
- 部署所需的 bundle、入口模块、API Level、签名状态和 HAP 输出目录改为从统一模型派生；工程能力面板复用同一模块与依赖口径，不再只扫描根下一层目录。
- 语义模型 schema 升级为 v2：结构化记录根/模块清单来源与解析错误，兼容 OHPM v1/v3 及 targetName 锁文件，并在依赖边同时保留声明约束、锁定版本和锁文件来源。
- 语义模型 schema 升级为 v3：全模块聚合 main pages、router map、权限 usedScene、SystemCapability 检查和 ArkTS/TS 跨模块 import，并生成带清单或源码位置的工程关系边；旧页面摘要改由该图派生。
- 语义模型 schema 升级为 v4：补齐产品 API Level、runtime OS、根与模块 build mode、apiType、设备类型、脱敏签名完整度和相对默认产品的差异字段。
- Agent 文件变更接入语义模型增量缓存：只重解析所属模块，沿 OHPM 依赖与真实 import 反向计算受影响模块、产品和建议验证；根结构变化或缺少缓存基线时安全回退全量解析。
- Workspace 工程分析新增可追溯产品矩阵、模块产物/Ability、清单状态与关系证据，并提供只读的文件变更影响预览，逐模块解释直接变化、OHPM 依赖、真实 import 或工程结构传播来源。
- `build_project` 升级为 environment → dependencies → build → artifacts 可恢复工作流：按统一模型核对并可自动安装 OHPM 依赖，持久记录工程指纹与脱敏 checkpoint，构建成功后必须发现 HAP/HSP/HAR 才完成。
- Hvigor/ArkTS 错误统一补齐源码位置、数字或命名错误码、构建阶段和根因类别，支持从 task 行继承阶段及读取下一行 `Error Message:`；Agent 错误证据与 Workspace 卡片使用同一结构。
- 构建失败新增日志—语义模型联合专项诊断：识别依赖版本冲突、缓存/完整性损坏、SDK 缺失、签名失败和 API Level 不兼容，并输出置信度、脱敏证据、自动恢复边界与顺序化修复步骤。
- `build_project` 新增影响驱动的构建计划：接受产品与变更文件约束，沿依赖/import 闭包选择各产品的最小顶层产物，分别调度 HAP/HSP/HAR 任务，并把确定性目标集合纳入恢复键。
- 构建成功新增持久 HAP/HSP/HAR manifest：记录内容 SHA-256、双时间戳、模块/产品/mode、来源 Hvigor step 和分级签名证据；文件不可读或清单无法落盘时不再把 artifacts 阶段标为成功。
- `deploy` / `deploy_all` 默认改为 manifest 驱动：部署前复验工程指纹、路径边界、内容哈希与签名结构，只自动选择唯一的最新可信 HAP；跨产品/模块或同时间多候选会列证据并要求显式确认。
- 设备列表升级为统一状态快照：同时提供 hdc 原始/归一连接状态、授权、系统/API Level、ABI、物理屏幕、证据化能力与观测时间；Workspace 和 Agent 使用同一数据源，在线探测并发且受超时约束。
- 部署闭环统一接入设备状态与能力门禁：显式设备也会复验连接/授权/安装/Ability/Hilog 能力；安装后启动失败会先留存日志证据，仅对本次新装应用执行补偿卸载并复验结果，覆盖安装不做破坏性误删；独立启动与卸载也增加状态确认。
- HarmonyOS 构建与运行证据接入 Agent 持久 Run 事件流：构建计划/结果/产物、逐设备安装与状态、Hilog、ArkTS 异常、Native 崩溃、AppFreeze/ANR 使用同一 `run_id` 和单调序号；后台旧监听受现有 Worker 租约 fencing 约束。
- `deploy_all` 增加 `serial|parallel` 多设备策略：串行固定逐台执行，并行缺省上限 2、硬上限 4，不再一次性 spawn 全部设备；设备结果确定性排序，并以逐设备事件和批次汇总写入当前 Run。
- `deploy_all` 恢复执行按 HAP 内容哈希复用父 Run 的逐设备成功证据：只重试失败或未执行设备，产物变化时自动重新部署全部目标，避免恢复操作重复安装已成功设备。
- `run_ui_flow` 打通操作、UI 树、关键页面断言与截图证据：支持 text/type/id/bundle 的存在/不存在和精确/包含匹配；操作或断言失败现在真实返回失败，`smoke_test` 不再把失败步骤误判为通过，并将证据路径写入当前 Run。
- `run_perf_benchmark` 补齐 Ability 启动状态确认、CPU/内存均值与峰值、电量变化、温度、FPS 和可信 HAP 包体积；前置 UI 流程失败不再产生无效基准，结果与可用性证据写入当前 Run。
- 真机韧性场景覆盖离线/授权门禁、安装身份冲突、权限拒绝、后台恢复和弱网：未授权不再误归签名错误，已有应用不自动卸载；权限撤销、后台回前台与网络 qdisc 设置/恢复均留下 Run 证据。
- 本机 SDK API 索引升级为文件级增量更新：未变声明复用、变化声明重扫、删除声明失效；索引同时覆盖类型、全部权限/SystemCapability、引入版本和废弃状态，并提供反向查询与刷新统计。
- 本机声明、官方变更与官方参考检索统一绑定当前工程 product 的 compile/compatible/target API 和已装 SDK；结果逐项标记可用、需运行时守卫、高于编译 SDK、废弃或移除，仅在 `@useinstead`/官方证据明确时提供替代。
- ArkTS 构建错误新增 API 证据映射：从错误提取类型/模块符号，关联当前产品 API Level、本机 `.d.ts` 官方定义和官方版本变更，并把可审计证据与恢复步骤写回源码定位和同一次 Run。
- `check_sdk_alignment` 升级为工程一致性审计：扫描 SDK import 并核对 API Level、精确权限、SystemCapability 守卫、设备类型、入口 Ability、permission usedScene 与产品模块归属；确定性问题、风险和降级提示分级输出且不自动篡改配置。
- `search_api` 新增 Android/Web/TypeScript 迁移模式：常见实现按架构语义映射到 HarmonyOS 候选，并用当前工程 API Level、本机 SDK 模块/符号和官方来源逐项标记 verified/conditional/unavailable/unverified，附风险边界与完整验证闭环。
- ETS 写入后的验证计划升级为强制闭环：最后一次写入之后必须依次取得本机 SDK/一致性审计、逐文件无错误 LSP、lint、测试、Hvigor 构建和最终 diff 证据；缺少任一必需步骤时统一执行循环保持 Verify，删除文件不会产生不可达 LSP 门禁。
- `environment_check` 新增 SDK/官方资料来源证明：统一展示本机 `.d.ts`、官方 API 变更与参考库、OpenHarmony 文档镜像的来源、版本、更新时间、条目和覆盖率，并把超过 30 天、缺失来源或缺少版本的索引显式降级，禁止作为生成代码的唯一依据。
- `ohpm_search` 升级为采用前包审计：用官方 registry 元数据比较显式或工程锁定版本与 latest，按工程 compatible API 核验包声明，分类许可证，并检查完整性摘要、废弃状态、安装期脚本和外部来源依赖；registry 没有漏洞公告证据时明确标为未知。OHPM 候选列表同步展示许可证。
- `get_project_info patterns=true` 新增 GitHub/Gitee 鸿蒙开源工程模式分析：绑定脱敏 origin、分支和 commit，只用语义模型与精确源码证据提取模块化、product、路由、Ability、依赖、状态、网络、存储、测试、Native 和多设备模式，并逐项给出适用边界、复用步骤与风险；扫描有确定性上限、不跟随符号链接且不执行第三方代码。
- `search_knowledge` 建立统一鸿蒙生态知识记录：团队经验之外可按 API Level、设备类型和错误指纹检索三方包兼容规则、常见错误与设备差异，每条记录绑定适用条件、回归来源、验证状态和未知边界；`ohpm_search detail=true` 将实时 registry 审计转换为同一版本化记录。
- `environment_check path=...` 新增 DevEco 公共配置互操作报告：对 AppScope、product/module、OHPM、Hvigor 和 manifest 生成确定性配置指纹，明确忽略 `.idea` 与 `local.properties` 私有内容，并只用字段路径提示机器绝对路径和敏感配置，不输出值或依赖 IDE 私有状态。
- 发布安全域改为逐次显式审批：release 构建、签名参考、OTA、凭据读取及发布/签名命令不能被 allow-all、项目/会话白名单或历史授权绕过；审批参数先统一脱敏，敏感调用失败后采用人工恢复。
- `copy_signing_from` 只允许授权根内的非敏感签名元数据和工程内材料引用，密码/令牌/私钥字段与目录外材料直接拒绝，未知字段按白名单丢弃；应用市场专用发布能力在满足同一治理契约前保持关闭。
- Skill manifest v1 新增独立 SemVer、HarmonyAgent 兼容范围、权限枚举、兼容状态和 `SKILL.md` 内容哈希：旧 Skill 明示为 `legacy_unverified`，不兼容项保持禁用，导入后内容漂移会阻止指令注入与调用；Skill 声明不能扩大现有工具权限。
- 内置能力包新增 schema/version、最低 Agent 版本与 `read_only|project_write|device_write|delivery` 权限上限，选择策略可独立于工具协议演进。
- 新增项目级工作流模板 v1：支持校验、导入、列出、启用、禁用和 SemVer 升级；逐步骤核对已注册工具、验收条件与权限清单，拒绝递归模板和不兼容 Agent 版本。
- 工作流导入/升级必须逐次显式审批；升级只接受更高版本，新增权限还需单独确认权限差异，旧版本归档在项目 `.deveco-agent/workflow-templates/history/` 供人工恢复，模板不会因导入或启用而自动执行。
- MCP 改为显式项目授权：旧配置升级后默认未授权，全局配置仅作可克隆模板；只有与当前项目精确绑定、启用且配置工具/目录/网络/凭据白名单的实例才进入 Agent。
- MCP 工具发现与实际调用双重核验授权，路径参数限制在项目相对根，deny 网络策略阻止网络地址参数且不注入代理；Agent 子进程清空继承环境，只传最小运行变量及明确批准的环境变量，服务器卡片不再显示环境变量值。
- MCP 命令或环境配置变化会使授权失效并断开旧进程，授权变更写入脱敏审计；操作系统级沙箱、强制网络隔离和第三方来源治理留待 EC10。

### 长会话 Context V2（M1 基础）

- Agent 工具参数在统一执行入口增加 schema 级预检：返回 JSON 语法、对象形态、缺失必填项和未知字段的纠错建议；所有参数均不静默改写，令牌、证书、签名与设备标识等敏感字段会显式禁止自动修正。
- 阶段工具选择器升级为证据驱动排序：在能力包先验上结合近 90 天成功率、平均耗时、预计结果 token 成本、副作用等级，以及当前 HarmonyOS 工程、Git 仓库和设备可用性；每轮只暴露得分最高的 32 个工具并记录可解释排名。
- 新增统一执行循环状态机，将目标契约、可验证计划、阶段最小工具集、真实执行证据、独立验证和最终验收收敛到同一快照；阶段变化持久记录为 `workflow.stage` 事件并在每轮重新注入，写入成功不能跳过验证门禁。
- 文件变更会按真实成功轨迹自动生成验证计划：ArkTS/ETS 选择格式化、lint、测试、Hvigor 构建和 diff；通用代码选择格式化、静态检查、测试、构建和 diff；文档改动至少核对 diff。失败写入不会产生虚假验证范围。
- 部署、Ability 启动、Git 提交/推送/合并、数据库迁移、密钥与知识库写入、HTTP 非只读请求新增写后读确认矩阵；部署和 Git 验收必须绑定时间顺序正确的后续状态读取，写入工具自己的成功文本不再自证完成。
- `compose` 多步工具流升级为可恢复逻辑事务：成功步骤写 Durable checkpoint，主步骤失败可走 fallback 降级，整体失败按逆序执行显式补偿，并列出未补偿副作用供人工恢复；禁止嵌套组合事务，未处理失败不再返回伪成功。
- 新增分层上下文模型：任务快照、来源化事实、产物引用、摘要覆盖游标和显式 token 预算。
- 新事实与旧事实冲突时保留历史版本并标记失效；Context 摘要不再被设计为文件、Git、工具或设备状态的唯一真源。
- 新增 Context 投影检查点和失效 epoch，并兼容读取现有会话摘要与任务账本。
- 聊天循环每轮从 Durable Run、执行步骤和来源化事实重建结构化上下文；摘要按消息/事件游标双写检查点，读取失败自动回退旧路径。
- 构建、Git、设备工具结果和产物自动进入 Context 投影；文件修改、分支切换、项目标识变化与设备副作用会使相关旧事实失效。
- Workspace 上下文状态条可展开查看当前目标、分层 token 预算、摘要覆盖游标、事实及产物来源。
- 热上下文新增最近消息、当前错误、活跃文件和待用户确认项；审批、计划审查与 Agent 提问均持久化请求、Owner、超时和终态，重启时只收敛已失联 Run，绝不自动批准。
- 自动与手动压缩后执行摘要—事实对账，附加机器生成的权威事实块；摘要与失败构建/测试、未完成 Run 或待审批状态冲突时记录纠偏审计。
- 新增 120 条消息压缩、SQLite 关闭重开和事实冲突换代回归测试。
- 项目长期记忆升级为 Context V2 项目层，补充架构/构建命令/模块职责/用户偏好分类，以及来源、可信度、版本、确认、固定和显式失效条件。
- 分支、项目身份、文件路径和设备副作用会按记忆声明的条件精准失效旧知识，并在记忆面板保留来源、版本与失效原因供解释。
- 关键消息、人工决策、活跃文件和验收条件可持久固定为权威上下文，跨压缩保留并参与摘要事实对账；原消息置顶入口同步 Context V2。
- 上下文达到主动压缩阈值、超限重试或摘要事实冲突时提供明确通知；恢复核验继续通过进度与错误状态显式反馈。
- 重启后从 Durable Run、步骤、Context 快照和事件游标恢复任务；恢复计划先核验文件、Git、产物、设备及外部状态，持久队列新增安全暂停、继续和取消控制并记录审计。
- 恢复任务时支持增量追加、明确移除和整体替换目标要求；目标契约差异进入事件与审计，旧目标下未完成且不再适用的计划项自动取消，“暂不推送”等否定表达不再误生成验收要求。
- 会话可从消息、检查点、构建失败或 Git 提交锚点创建持久分支；合并严格限制为固定决策、验收条件、产物引用和来源化验证事实，不拼接消息或摘要。
- 子 Agent 委派升级为协议 V2：限定上下文引用、工具范围与嵌套深度，明确不复制父会话全文；返回值统一为带验收、产物、证据、阻塞项和错误的 `SubAgentResultV2`。
- 完成长会话 M2 自动化验收：120 条消息重开恢复、四小时等效 checkpoint/lease 恢复、目标变更、来源追溯与副作用防重放均形成可重复测试证据。
- 工具结果统一为可扩展的 `ToolResultV2`：所有注册工具稳定输出状态、修改、产物、验证、恢复、建议与错误信封，并兼容旧 V2 记录及未知未来字段。
- 工具执行契约补齐副作用、幂等、超时、取消、重试、审批与恢复元数据；Tool Worker 超时改由契约驱动，未知 MCP 工具采用始终审批的保守写入策略。
- 成功与失败的长工具输出统一外部化为受保留策略管理的产物，模型只接收有界头尾摘要和读取引用；`ToolResultV2` 同步记录产物路径。
- 文本与 JSON 脱敏收敛到统一入口，覆盖 token、证书/私钥、签名材料、敏感环境变量、连接口令和设备唯一标识；MCP 错误、长输出产物、工具审计与人工交互不再存在旁路。
- 验证器和恢复动作进入工具契约真源：验证证据不再依赖结果层硬编码，所有副作用工具均声明快照恢复、Git 补偿提交、重新部署、核验后补偿或人工恢复策略。
- 新增项目理解、编译修复、功能开发、重构、构建部署、设备诊断和 Git 交付 7 个能力包；系统提示与原生 tool schema 共用有界选择器，每包声明最小工具集、顺序、停止条件和验收。
- 每轮模型请求根据持久工具证据在 explore/modify/verify/deliver/recover 阶段间切换，动态注入最多 32 个阶段工具；Git 推送仅在验证通过且目标明确要求交付后开放。
- 可靠性面板新增工具治理清单：按窗口识别高失败率与真实长期未使用工具，并列出保守的功能重叠候选及修复、隐藏、合并审查建议。
- 修复 `062_tool_execution_threads.sql` 未登记到统一迁移清单的问题，确保已有用户升级时真实应用 Tool Worker 线程字段。

### 目标契约与证据验收

- 用户目标编译为结构化 `GoalContract`，识别修改、验证、构建、测试、部署、commit 和 push 等必需条件。
- 工具结果转为结构化证据，记录产物、验证范围、错误、补偿策略、指标和 evidence digest。
- 模型只能申请完成；运行内核依据真实工具轨迹裁决。修改后的验证必须发生在最后一次写操作之后，缺证据会自动进入补救循环。
- 达到补救预算仍未满足契约时，Run 收敛为 `interrupted/continuation_required`，不再把自然语言完成声明当作成功。

### Durable Run、调度队列与 DAG

- `agent_runs` 扩展目标契约、动态预算、租约、恢复信息与质量快照；Run 终态不可逆。
- 新增持久化 `agent_task_queue`，支持优先级、claim、退避重试、checkpoint、resume token、并发键和 tenant。
- 新增 Agent DAG 节点/边：主任务和子 Agent 记录依赖条件、失败策略、独立尝试与验收结果；根验收合并子节点证据。
- 新增 execution step 协调与副作用感知恢复：读取可安全重试，写入/命令/部署先验证效果，无法判定时要求人工确认。

### 多进程 Agent Worker

- 每个桌面进程登记唯一 Worker、PID、主机、容量和心跳；启动第二实例不会中断仍健康的第一实例任务。
- 队列 claim 生成 lease token 与递增 epoch，checkpoint、续租和终态写入执行 Owner fencing，旧 Worker 的迟到写入被拒绝。
- 心跳扫描仅回收真正过期或失联 Owner；新增真实进程崩溃 E2E 覆盖认领、进程退出、租约过期和接管恢复。

### Tool Execution Kernel

- `tool_runs` 增加协议版本、结构化结果、幂等键、执行 Worker、租约、尝试、验证状态、恢复次数与 outcome commit 时间。
- 副作用工具采用 prepared → running/verifying → committed 语义；同 Run 的重复副作用按幂等键阻止，迟到结果按 lease fencing 丢弃。
- 实际工具 future 迁到命名专用 OS 线程执行；线程 panic 由 `catch_unwind` 隔离，不拖垮主进程。
- 调用方超时/取消但线程仍运行时标记 stuck，后台同时扫描租约过期调用；控制面新增 `stuck_tools` 指标和 Worker 线程身份。
- 增加卡死线程、不可取消迟到结果、输出洪泛和真实孤儿进程隔离测试；Unix 进程树清理先允许包装器回收已终止子进程，再兜底强杀，避免遗留僵尸 PID。
- 工具质量指标新增成功率、参数错误率、超时率、重试率、取消延迟和平均耗时，并在最终验收后区分直接贡献与“成功但未推进验收”的调用。
- 工具 SLO 新增副作用重复、能力包外错选和无效成功上限；可靠性面板可按工具、能力包、模型、项目和协议/应用版本比较成功率、贡献率与耗时。
- 新增工具协议版本目录和生产者版本维度：V1 历史记录保持只读兼容，V2 保留未知未来字段，后续不兼容变更必须使用新 schema 版本和显式迁移。
- `ToolResultV2` 增加向后兼容的影响说明，失败结果统一给出原因、真实状态影响、已完成部分和恢复下一步；阶段门禁新增 12 个高频工具故障协议矩阵及典型任务裁剪后可完成性测试。
- 新增工具线程 panic、进程崩溃、副作用恢复与重复执行防护 E2E。

### 可靠性控制面与质量门禁

- 新增 SLO policy、告警、审计事件、配额和评测历史；成本页展示验收率、质量分、恢复率、结构化证据覆盖率、队列/DAG、Agent Worker、Tool Worker 和卡死工具。
- CI 在 macOS/Windows 上新增 reliability、Execution Kernel、多进程 Worker crash 和 Tool Worker crash E2E gate。

### 文档校准

- 重写架构文档，以 Rust 后端 Agent 主循环和双层执行内核替换已过时的“前端 TS 编排”方案。
- README 代码规模更新为 198 工具、29 个 Agent 顶层模块、29 个工具文件、33 个命令模块、36 个服务模块、281 个 IPC 入口、68 个迁移和 14 个页面。
- 明确能力批次版本与应用 manifest 版本的口径差异；本批不修改应用发布版本，`package.json`、Cargo 和 Tauri manifest 仍为 `2.0.0`。

### 修复

- 统一首选 HAP 输出目录与递归 fallback 的产物排序：`-signed.hap` 优先于较新的未签名包，避免部署阶段误选不可直接安装的 unsigned 产物；新增回归测试。
- 管理侧栏移除硬编码 `v0.1.0`，改为通过 Tauri `getVersion()` 显示当前 manifest 版本，并保留 `2.0.0` 启动 fallback。

### 迁移

| 编号 | 内容 |
|---|---|
| `057_agent_governance.sql` | 目标契约、补救、Run 租约和质量快照 |
| `058_reliability_control_plane.sql` | 结构化证据、调度队列、DAG、评测 |
| `059_execution_kernel_v2.sql` | 队列协议、工具协议 V2、SLO/告警/审计/配额 |
| `060_multi_worker_runtime.sql` | Agent Worker、lease token、claim epoch、尝试账本 |
| `061_tool_execution_kernel_v2.sql` | Tool Worker、执行租约、验证/恢复与尝试账本 |
| `062_tool_execution_threads.sql` | 工具线程身份与 stuck 计数 |
| `063_conversation_context_v2.sql` | 分层上下文、来源化事实、产物引用与摘要游标 |
| `064_pending_interactions.sql` | 审批、计划审查、Agent 提问的持久生命周期 |
| `065_context_reconciliation.sql` | 摘要与结构化事实的冲突检测及纠偏审计 |
| `066_structured_project_memories.sql` | 项目记忆来源、版本、确认、固定与条件失效 |
| `067_context_pins.sql` | 用户固定消息、决策、文件和验收条件 |
| `068_conversation_branches.sql` | 会话分支血缘与结构化合并清单 |

---

## v2.2 — 八仓库盘点落地：混合检索 + 时间旅行 + 定时提醒 + 跨会话引用（2026-08-20）

定位：对 8 个参考仓库（deepseek-harness / qwen-code / Qwen-Agent / langgraph / OpenHands 等）做全量盘点后的能力落地——检索、会话管理、任务编排、工具集各补一批高价值能力，工具集 **193 → 198**。

### 🔍 检索与记忆升级（desA，对齐 Qwen-Agent）

- **BM25 重排**：新增 `utils/tokenizer.rs`（中文 2-4 字滑窗 n-gram + 英文整词 + 停用词过滤）与 `utils/relevance.rs` Okapi BM25 索引（k1=1.2 / b=0.75，与 rank_bm25 一致）；`keyword_search` / 记忆检索结果从 SQL 字典序改为 **BM25 相关性重排**（标题双份注入近似位置权重 + 时间衰减 + 类别加权）。
- **front_page 置顶**：记忆注入预算充足时，最近更新的 2 条记忆无条件置顶（对齐 Qwen-Agent front_page_search），预算不足自动跳过。
- **RRF 融合**：embedding 向量检索与 BM25 关键词检索双路 RRF 融合（对齐 Qwen-Agent hybrid_search 的混合检索三件套）。
- **pitfall 加权前置**：构建错误修复任务中 build 类历史记忆加权前置，Agent 动手前先看到本工程踩过的同类坑。

### 🧭 会话时间旅行（对齐 langgraph checkpoint）

- **快照自动保存**（migration `051_conversation_snapshots.sql`）：每轮工具执行后保存状态锚点（可见消息 rowid + 账本 + 模型输出摘要），每会话上限 50 条，首轮无执行痕迹不保存。
- **双向恢复**：`restore_snapshot` 归档锚点后的消息（hidden，旧分支保留可回溯）、重现锚点前的归档段、账本写回快照时刻（续跑继承该点执行轨迹）；任务运行中拒绝恢复（防写消息竞态）。
- **前端时间线**：更多菜单 →「会话时间线」弹窗，快照点列表（标签/时间/工具数/当前标记），「回到此处」warn 确认后恢复并刷新消息/账本/审计留痕（`task.timeline`）。

### ⏰ 定时提醒（对齐 deepseek-harness schedule）

- 新工具 `schedule_create`（after / at / every 三类，错误码含 invalid_prompt / invalid_selector / not_future / frequency_too_high）/ `schedule_list` / `schedule_delete`；every 锚点推进（错过不枚举历史周期）。
- 新服务 `services/reminders.rs` + migration `052_reminders_feedback_terms.sql`（`message_reminders` 表）；lib.rs setup 30s 轮询派发到期提醒 → 会话队列注入（`inject_message`，session-local 不中断当前轮次）+ 桌面通知。

### 📊 消息反馈纠偏（A2）

- 点踩（dislike）消息内容高频词（词频 ≥2 取前 5）写入 `feedback_terms` 词袋；记忆注入前加载负反馈词袋，命中 ≥2 个不同词的记忆剔除不注入、命中 1 个的排到末尾——用户不期望的内容不再反复出现在上下文。

### 🛡 不变式守卫（A5）

- 新增 `agent/invariants.rs` 注册表（`Invariant { name, check }` + 静态数组 + `check_write` 统一入口），3 条不变式：`.env*` 前缀文件、8 种密钥/证书后缀（`.key/.pem/.pfx/.p12/.keystore` 等）、已存在的 `migrations/*.sql`（已执行迁移不可修改，新建允许）；`fs_tools::is_protected_file` 收拢为委托注册表，含 2 个测试。

### 🔗 跨会话引用（B6）

- `references_json` 支持 `conv:<id>` 前缀：历史重放时注入会话标题 + 摘要（`messages.summary` 非空优先，回退最后一条 assistant 内容，单会话 2000 字符 / 总 8000 上限）；前端 @ 面板追加会话候选（同项目、排除当前、标题模糊匹配、chat 图标），选中即把标题 + 最近内容插入草稿，与消息引用（Quote）同构。

### 🛠 流式健壮性加固

- **无产出静默超时**：连接保持但 60s 解析不到有效内容 → 保留已收内容自动续写（与截断续写同链路）。
- **产出前中断冻结重放**：流在输出任何内容前中断 → 冻结请求原样重发（≤5 次，对齐 DeepSeek-Reasonix 机制，模型无需重新思考、prompt 缓存不失效）。
- **工具循环检测**（对齐 qwen-code LoopDetectionService 轻量版）：连续相同调用（name+args）/ 连续同名调用（参数抖动）/ 每轮工具总数软硬上限，命中注入纠正提示，最多打断两次后收尾。
- **行动承诺假完成纠正**：模型宣布开始开发/仅输出方案计划但无任何工具标记时，注入纠正提示要求立即执行（上限防死循环）。
- **reasoning_content 多轮合规**：DeepSeek 推理模型携带 tools 的请求完整回传思考链（缺失导致 400/思考链断裂）；V4 thinking 模式 content 数组块解析（text 块进正文 / thinking 块归推理）；仅带工具调用的 assistant 消息回传 reasoning（纯文本回答不回传、不占输入预算）。
- **run_command 输出超限落盘**：响应超限时全文落盘 + 头尾采样 + `store_overflow` 路径标记，Agent 可按需读回完整输出。

### 🧰 其他

- `ui_focus` 工具（对齐 OpenHands canvas_ui_control）：Agent 产出后驱动 UI 聚焦（切换右侧面板 / 打开文件预览，L0 权限）。
- `memorize` 工具 + `replay_memories`（对齐 Qwen-Agent MemoAssistant）：从历史消息重放 memorize 调用重建键值状态，每轮作为 system 注入。
- 文件树面板：展开但缓存缺失时自动重新加载（刷新后已展开目录免手动再点）。
- logger 测试隔离修复（pid 复用残留文件导致偶发断言失败）。

### ✅ 验证

- `cargo check`：0 error / 0 warning
- `cargo test --lib`：**446 passed / 0 failed**（新增 reminders 2 + invariants 2 + 检索/协议若干）
- 前端 `tsc --noEmit`：通过

### 🔄 迁移要点

- 新增迁移 `051_conversation_snapshots.sql`、`052_reminders_feedback_terms.sql`（已执行库自动应用，无破坏性变更）。
- 工具总数 193 → **198**（+memorize / ui_focus / schedule_create / schedule_list / schedule_delete）；`TOOL_SPECS` 数量以 `src-tauri/src/agent/tools/mod.rs` 为准。
- `inject_references` 签名新增 `conn` 参数（conv: 会话摘要查询）；内部调用点已同步。

---

## v2.1 — 对话流转加固 + 极简留白 UI（2026-08-19）

定位：围绕"对话能否正常流转"做一次全面体检与修复，解决停止/删除/审批/错误态等边界场景的状态不一致，并把对话区视觉改为极简留白风格。

### 🐛 对话流转修复（后端）

- **停止语义修复**：用户点停止后，不再自动续跑排队消息（`stream_chat_body` 在 `stats.stopped` 时终止队列消费），避免"点了停止，过会儿 AI 又自己开始干活"。
- **删除运行中会话**：`delete_conversation` 改为先停止 + abort 后台任务（新增 `TaskRegistry::abort_conversation`）+ 释放项目锁，再删库，消除孤儿任务、继续写文件和项目锁长期占用问题。删除时同步清理 `tool_limits` / `task_guard` 进程内状态，修复内存随会话数单调增长。
- **审批/计划审查中停止**：新增 `InterceptKind::Cancelled`、`ApprovalOutcome::Cancelled`、`PlanReview.cancelled`，工具审批/计划审查等待期间点停止，现在按"停止"收尾（`chat-stopped`），而非被当成"拒绝"导致任务继续跑一轮或显示为正常完成。串行与批处理工具路径均已覆盖。
- **任务看门狗**：`TaskRegistry` 统一登记所有 `stream_chat` 任务，8 分钟无心跳 / 40 秒停止未生效时强制 abort 并 emit `chat-error`；`stream_once` 内按阶段（发送→首字节→流式→解析）高频 touch。
- **新增迁移 `050_task_ledger.sql`**：持久化 `task_runs.target_text / target_passed / target_evidence`。
- `chat-done` 事件新增 `user_message_id` 字段，供前端替换乐观占位。

### 🐛 对话流转修复（前端）

- **错误态与流式残影共存**：出错时清 `conversationId` / `startedAt`，打字光标/三点动画立即消失，只保留已生成内容 + 错误卡。
- **乐观 user 消息 ID 不替换**：`chat-done` 用真实 `user_message_id` 替换 `local-` 占位，当前会话周期内的编辑/删除/分支重生成/Fork 立即生效。
- **停止兜底计时器误杀新任务**：`stopGeneration` 的 60s 兜底用 `startedAt` 代次 token 校验，停止后立即重发不再被旧计时器置错。
- **看门狗误杀后台审批会话**：改为按 `pendingConfirmations[convId]` 判断（含后台会话），而非仅当前会话视图数组。
- 乐观消息 ID 加随机后缀避免跨会话同秒碰撞；排队失败弹错误通知；`chat-done` 按完成会话自身 `project_id` 刷新列表；新增 `conversation-deleted` 事件监听（多端/LAN 删除时同步清理并切换会话）。

### 🎨 对话区极简留白样式

- 消息头模型/耗时/token/消息ID 徽章**默认隐藏，悬浮显示**；assistant 头像去紫色渐变改朴素圆点；用户气泡去彩边/阴影改中性背景。
- 工具卡 / 子 Agent / 计划卡 / 账本卡 / 任务过程条统一为**纯文字行 + 折叠**：去掉彩色背景、左侧竖条、图标底色块、阴影和完成脉冲。
- 思考块（ThinkingBlock）改左侧细竖线；错误卡弱化为中性边框；CSS 中 `.task-*` 类去背景/竖条/阴影。

### ✅ 验证

- `cargo check`：0 error / 0 warning
- `cargo test --lib`：**418 passed / 0 failed**（含 ask/guards/pipeline）
- 前端 `tsc --noEmit`：通过

### 🔄 迁移要点

- 新增迁移 `050_task_ledger.sql`（已执行库自动应用，无破坏性变更）。
- `delete_conversation` 命令由同步改为 `async`，签名新增 `app/cancel/lock/registry` 状态参数；LAN 服务改用同步内部函数 `delete_conversation_sync`，HTTP 行为不变（删除仍会级联清理运行中任务）。

---

## v2.0 — Agent Workspace 收尾（2026-08-16）

定位：从"Provider 切换器"升级为**完整 Agent Workspace**——工具集 117 → **191**，覆盖鸿蒙开发全链路；新增 9 个能力工具 + ToolError 结构化错误；命令面板与 i18n 同步落地；超大单文件按职责拆分。

### ✨ 新增（9 个 A 类工具 + 1 个错误体系升级）

| ID   | 工具 | 能力 |
|------|------|------|
| [14] | `log_query`         | hilog / runtime_log / faultlog 三源结构化查询（since / level / keyword / regex / 设备过滤），输出按时间聚合 + 命中段截断 |
| [23] | `docx_read`         | `.docx` 正文（纯标准库 `zip` + XML 流式解析，零依赖） |
| [26] | `audio_transcribe`  | 调本地 `whisper.cpp` 转写（自动定位 whisper 二进制 + ggml 模型） |
| [28] | `attach_debugger`   | `hdc shell debuggerd -p <pid>` attach + `aa debug` 回退；输出 PID / bundle / wait_secs 与下一步指引 |
| [29] | `step_debug`        | step / next / continue / interrupt / where / info 六动作调试驱动 |
| [30] | `memory_snapshot`   | take / list / diff 三动作；连续两次增长 > 10% 自动提示"疑似泄漏" |
| [36] | `ota_pack`          | 内置 `packagingtool.jar` → `.pkg` 打包（自动找 jar、可选 profile_path 注入签名） |
| [48] | `license_check`     | 扫 `oh-package.json5` / `Cargo.toml` / `pyproject.toml`，对照内置 allow/deny 黑白名单输出违规项 |
| [49] | `vuln_scan`         | 内置 10 个已知漏洞（lodash/axios/requests/spring/jackson 等），按依赖版本匹配，给出 CVE 与建议版本 |
| [65] | `ToolError`         | 7 类 category（network/permission/not_found/invalid_input/internal/timeout/conflict）+ 是否可重试 + 自动建议下一步；`run_tool` 出口自动套信封，零侵入覆盖所有 191 个工具 |

### 🛠 重构与拆分

- **工具集重组**：原 v1 的若干大工具拆分为更聚焦的变体（如 `lsp_*` 9 个、构建/部署/签名分项、调试 `attach/step/breakpoint` 独立），最终工具数 **117 → 191**。
- **TOOL_GROUP**：按 8 个域分组（`build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`），前端按组渲染与限额。
- **TASK_GROUPS**：与 TOOL_GROUP 对齐，限额与守卫按组生效（修复了"按工具名限额"导致热门工具被全局压制的问题）。

### 🎨 命令面板 + i18n（前端配套）

- 命令面板新增 **28 个高频工具 action**（`Cmd+K` 即时触发），覆盖调试 4 / 重构 5 / 构建 2 / 部署 1 / 安全 4 / 知识 4 / 数据 2 / 治理 5 / 多模态 3。
- 中英文 `i18n` 增加 30 条工具标签（zh + en 各 30 条），前端 fallback `t('toolToolName')` 兼容未命中。

### 🧹 代码结构清理

- `agent/tools/quality_tools.rs` 由 **2400+ 行单文件** 拆为 facade + 4 个子文件，**按方法完整切片**（不按行数切，保证每个 `fn` 跨多行签名 + 函数体完整落在同一文件）：

  | 子文件 | 工具数 | 函数数 | 内容 |
  |--------|-------:|------:|------|
  | `quality_metrics.rs`  | 7 | 15 | code_metrics / metric_export / log_aggregate / log_query / memory_snapshot / snippet_insert / replay_trace + 7 个 helper + `FileMetrics` / `SOURCE_EXTS` / `SKIP_DIRS` |
  | `quality_security.rs` | 4 |  9 | obfuscate / sandbox_exec / license_check / vuln_scan + 5 个 helper |
  | `quality_runtime.rs`  | 6 | 11 | api_test / api_mock / api_health / attach_debugger / step_debug / ota_pack + `MockRoute` struct + 4 个 helper + `hdc_shell` |
  | `quality_media.rs`    | 2 |  5 | docx_read / audio_transcribe + 3 个 helper |

  拆分原则：
  - `pub use module::*` 在 facade re-export，对外 `quality_tools::code_metrics(...)` 调用方式零变更。
  - helper 跟随"主消费者"所在文件（如 `parse_dep_line` 跟 `license_check` 走 security）。
  - `pub(super) async fn` → `pub async fn`（`pub use` 不能 re-export 私有项）。
  - `super::xxx` → `crate::agent::tools::xxx`（facade 不可见，需走绝对路径）。
  - 跨文件共享的常量（如 `SKIP_DIRS`）按"谁需要谁就近复制"，避免反向依赖；确实跨多处用的，`scanner.rs` 上加 `pub`。

- 根目录清理 **59 个调试/分析脚本**，统一归档到 `scripts/legacy/`（含 11 个 Python 处理脚本 + 48 个旧日志/测试产物）。
- `.gitignore` 增补 `scripts/legacy/` / `__pycache__/` / `*.pyc` / `*.log` 等规则，避免误提交临时文件。

### 📚 文档

- `README.md` 从 1642 字节扩到 12k+ 字节，重新定位为"Agent Workspace"，补全工具清单、能力矩阵、命令面板使用、安全治理、内置运行时说明。
- `docs/tool-enhancement-backlog.txt` 升级为 v2 完成态（56/76 兑现，3 项外联 figma/feishu/jira 按用户要求暂缓）。
- `docs/ARCHITECTURE.md` 同步更新：拆出 quality 子文件后的模块图、TOOL_GROUP × TASK_GROUPS 关系表。

### ✅ 验证

- `cargo check --lib`：**0 error / 0 warning**
- `cargo test --lib`：**346 passed / 0 failed**（其中 7 个为新 ToolError 单元测试）
- 拆分前后 `quality_tools::xxx(...)` 调用点 **0 处需要修改**（facade re-export 兼容）

### 🔄 迁移要点

- 无破坏性变更。`quality_tools` 公共 API 100% 兼容，外部 import 不需修改。
- `agent::scanner::SKIP_DIRS` 由 `const` → `pub const`（被 `quality_security` 借用），如外部代码依赖其私有性请注意。
- 命令面板默认展示顺序变化：高频工具置顶，长尾工具折叠到二级菜单。

---

## v1.0 — 初版提交（2026-08-14）

定位：HarmonyOS 桌面 AI 编程 IDE 雏形，**117 个 Agent 工具** + 多 Provider 路由 + 内置运行时。

### 基础能力

- **AI Agent 内核**：多轮对话、子 Agent 派生（`spawn_agents`）、任务计划（`plan_task`）、TodoWrite 进度跟踪、`undo_edit` 撤销栈、跨轮诊断记忆、`ask_user` 主动提问、后台任务（`run_command --background`）、运行时日志（`hdc shell hilog -L E`）。
- **鸿蒙深度集成**：hdc 设备管理 / 真机无线连接 / 模拟器启停 / hvigor 构建 / ohpm 依赖 / faultlog 崩溃归因 / hilog 实时回流 / 多模块工作区识别。
- **多 Provider 路由**：华为 / 智谱 / 通义等多家 LLM 接入 + 本地 HTTP 代理 + 熔断器 + 自动 failover + 费用追踪 + 请求日志。
- **API 知识库**：内置 HarmonyOS API 索引（向量检索 + 符号索引） + 跨版本 diff + 兼容性扫描 + 用户笔记。
- **安全治理**：工具调用白名单 / 工具限额 / 任务守卫 / 预算控制 / 权限管理 / 审批拦截流水线（pre/post hooks）。
- **内置运行时**：自带 Node + JDK + Git 运行环境（`src-tauri/runtime/`），用户机器无需预装开发环境。
- **代码理解**：分级扫描（`check_code` / `deep_scan` / `codebase_search` / `get_symbol_details`） / 符号索引 / 文件系统工具集。
- **生态能力**：MCP 服务器管理 / Skill 启停 / 鸿蒙官方文档检索 / Web 搜索与抓取 / 知识库导入导出。

### 关键模块

| 模块 | 行数（v1 末） | 说明 |
|------|-------------:|------|
| `agent/tools/mod.rs`        | ~4200 | 工具注册表（TOOL_SPECS / TOOL_GROUP / 191 个工具 dispatcher） |
| `agent/tools/fs_tools.rs`   | ~1500 | 文件读写 / 搜索 / 折叠 / gitignore |
| `agent/tools/build_tools.rs`| ~1200 | hvigor / ohpm / 签名 / 部署 / 产物分析 |
| `agent/tools/cmd_tools.rs`  | ~ 700 | run_command 危险命令黑名单 + 沙箱 + 后台任务 |
| `agent/agent_board.rs` 等   | 各 200-600 | Agent 编排、反思、记忆、会话事件、任务队列 |

### 已知遗留（v1 末 → v2 修复）

- 对话 SSE 流式响应在 chunk 边界切多字节字符 → `U+FFFD` 永久入库（v1.1 修：字节缓冲整行解码）
- `list_dir` 不遵循 `.gitignore`（v1.1 修：含子目录 + 子模块规则）
- `read_file` 注释占比高时无折叠，淹没代码（v1.1 修：连续长注释块折叠为一行摘要）
- gitignore 运行时静默失效（`canonicalize` 加 `\\?\` 前缀与 `normalize` 不一致，v1.1 修）
- 大量调试/分析脚本散落根目录（v2 修：归档到 `scripts/legacy/`）

---

## 版本对照速查

| 维度 | v1.0 | v2.0 | 增量 |
|------|-----:|-----:|-----:|
| 工具数（TOOL_SPECS） | 117 | 191 | **+74**（+63.2%） |
| 新增工具 | — | 9 + ToolError | — |
| TOOL_GROUP 域 | 3 | 8 | +5 |
| 命令面板 actions | 0 | 28 | +28 |
| i18n 工具标签 | 0 | 30 条 × 2 语言 | +60 |
| 文档（README + ARCHITECTURE + backlog） | 60+1248+0 | 12000+1500+700 | +10× |
| cargo test | 282 passed | **346 passed** | +64 |
| 编译错误/警告 | 0/0 | **0/0** | 持平 |
| 根目录调试脚本 | 59 | 0 | -59 |

---

## 维护说明

- 工具总数以 `src-tauri/src/agent/tools/mod.rs` 中 `TOOL_SPECS` 数组长度为准（当前 201）。
- 任务分组以 `TASK_GROUPS` 常量为准（当前 8 个：`build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`）。
- `quality_tools::*` 通过 facade 暴露，**禁止**直接 import 4 个子文件（`quality_metrics` 等）—— 内部模块，外部耦合面随 facade 走。
- 任何对工具的"按行数切分"禁止。**必须按方法完整切片**，签名 + 函数体在同一文件内。脚本辅助可见 `scripts/legacy/_split_quality.py`。
- CHANGELOG 任何变更在 commit message 里写 `docs(changelog): <一句话>`，不要直接编辑本文件然后 commit `docs:`。
