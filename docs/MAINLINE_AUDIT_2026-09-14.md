# 主线实现与验收盘点（2026-09-14）

基线提交：`8fdea95`。本轮是代码、文档与本机回归盘点，不实施功能修复、不提交或推送代码，不操作真实设备、不调用付费模型、不使用 Docker。

> 上述为 9 月 14 日盘点阶段的边界；用户随后授权继续实现。后续代码落实状态见第 9 节，历史发现不代表仍全部未修复。

## 1. 总结

**项目已有较完整的 Agent 工程底座，但整个主线尚未完成，也不只是“剩最后验收”。**

- 文件/结构索引、目标与计划、工具治理、恢复、安全审批等已有大量实现和自动化证据。
- 最近 OTA 审批 P0—P6 是一个子链路的加固，不等于整个 Broker、设备开发闭环或主线完成。
- 尚有明确实现缺口：Windows 原生沙箱生命周期、原生资源限制、桌面统一 IO run-loop、Java 类型语义、独立签名/证书/市场发布等。
- 还有验收缺口：真实模型成功率、真实混合大仓语义召回率、真机/模拟器/OTA 工具链、发行新机器验收。
- 已确认恢复工具卡丢失撤销身份、撤销记录未进入统一审计时间线等接线缺口，见第 4 节。

不报单一“完成百分比”：不同文档口径重叠，P0—P6 是子任务编号，不是产品完成度；单测数也不是用户任务成功率。

## 2. 本轮实际验证

| 检查 | 结果 | 能证明 / 不能证明 |
| --- | --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib --quiet` | 1021 项：1012 通过，9 忽略 | 当前库回归通过；不含 ignored 环境验收 |
| `cargo test --manifest-path src-tauri/Cargo.toml --test tool_worker_crash_e2e --quiet` | 3 通过 | Tool Worker 故障协议集成；不是设备 E2E |
| `cargo test --manifest-path src-tauri/Cargo.toml --test worker_crash_e2e --quiet` | 3 通过 | Worker 崩溃集成；不是完整桌面恢复验收 |
| `npm test -- --reporter=dot` | 14 文件、116 项通过 | 前端测试覆盖范围内行为正确 |
| `npm run lint` | 通过 | 无 ESLint 报错 |
| `npx tsc -b --pretty false` | 通过 | TypeScript 类型检查通过 |
| `npm run build` | 通过，bundle gate 通过 | Web 生产构建通过；不是 Tauri 安装包验收 |
| `git diff --check` | 通过 | 无补丁空白错误 |

构建仍有大 chunk 警告：Home 692.7/750 KB、Markdown 1500.8/1550 KB、main index 544.4/575 KB，接近当前门禁预算。Rust 非测试库构建仍报告部分 dead-code 警告；没有将“退出码 0”表述为“全部零警告”。

本轮没有重跑百万文件生成基准，避免大量临时数据；没有启动真实 Provider、DevEco/hdc、系统沙箱 smoke、真实桌面 GUI 或 Windows/macOS 安装包验证。历史基准是文档证据，不冒充本轮复测结果。

## 3. 主线能力矩阵

| 能力域 | 实现与接线证据 | 当前判定 | 仍缺少的退出条件 |
| --- | --- | --- | --- |
| 自定义目标、计划驱动 | `chat.rs` 的 GoalContract、activate_approved_plan、继承 approved_plan；runtime 持久计划 | 已实现核心链路 | 真实模型“计划→多步执行→中断恢复→交付”完整轨迹 |
| 长会话恢复与执行治理 | `kernel_executor.rs`、桌面 checkpoint/cursor、预算继承、两组 crash E2E | 基础实现与本机回归充分 | 桌面 IO adapter 完整统一、真实长任务验收 |
| Headless Agent | `HeadlessIoPort` 已交给 `KernelIoRunLoop::run`；手动 workflow、评测契约 | 核心实现，生产证据不足 | 成功真实 trial 的 manifest/trajectory/patch/report/cost；公开基准 |
| 百万仓库文件可达性 | `symbol_index.rs` 全库目录、deferred、后台渐进解析、watcher；`scip_index.rs` | 已实现，生成仓数量级有历史证据 | 真实混合仓全量完成耗时、Recall@k、外部大批改动、前台查询 P95 |
| 结构理解与影响查询 | `repo_query_tool`、AST/SCIP/LSP 边、分页与覆盖状态 | 部分语义能力落地 | 统一依赖重排 planner、跨语言/跨模块正确率；必要时再分片 |
| 按块修改与注解保护 | `code_mutation.rs`、结构句柄、多节点/文件事务及 UI 门禁提示 | 可用基础保护，不是编译语义保证 | Java JDT/Javac import/override 联动；真实 Dart/Java 变更验收 |
| 工具选择与验证循环 | capabilities/ranking、生产链选择 64 候选后取最多 32；共享契约与验收 gate | 已实现阶段选择 | 8—20 常驻目标、完整延迟 schema/程序化编排与同模型 A/B |
| 无 Docker 原生隔离 | `sandbox.rs`、显式 native run_command、SandboxCapabilityPanel | 部分完成 | Windows token/profile/ACL；CPU/内存/PID/磁盘；默认启用及跨平台逃逸验收 |
| Host Capability Broker | 类型化能力、固定 argv、claim/fencing/恢复、设备/部署接线 | 原型超出初始范围，但未完成全部契约 | 非 OTA 审批/影响契约统一；真实命令兼容与效果证明；完整签名/发布能力 |
| OTA 审批安全链 | scope、只读副本、v3 凭据、持久撤销、桌面单调用撤销 | 核心链路实现，仍有接线缺口 | 修复第 4 节；真实打包/签名/升级；撤销/派发/发布竞态 |
| HarmonyOS 工程与设备闭环 | 工程/SDK/构建/部署/UI/性能工具与固定录制场景 | 大量实现，实时验收不足 | 真机/模拟器版本矩阵：离线、重连、安装冲突、启动、UI、日志、恢复 |
| 评测与发行 | 固定 25-ID 清单、eval harness、release workflow、更新签名 | 基础设施已建 | HarmonyBench 50/100+、真实 SWE 报告、平台代码签名/SBOM/provenance/新 VM |

关键源文件：`src-tauri/src/commands/chat.rs`、`src-tauri/src/agent/{kernel_executor,headless_driver,headless_runtime,capability_broker,broker_approval,ota_scope,ota_inputs,sandbox}.rs`、`src-tauri/src/services/{symbol_index,scip_index}.rs`、`src-tauri/src/agent/tools/{code_mutation,capabilities,mod}.rs`。

## 4. 已确认的缺口与待核验风险

### A1 — P2：恢复工具卡遗漏真实 callId，撤销入口缺失

证据：`src/stores/slices/chatSlice.ts:828` 实时工具事件同时设置 UI id 与 `callId`；恢复的 `restoredToolRuns`（约 1537 行）和 `refreshedTools`（约 1652 行）只设置 UI id，没有 `callId`。`toolRuns.tsx:222` 必须有 callId 才显示撤销按钮。

影响：即使 durable step 有真实 `external_id`，恢复/切换会话后重建的运行中 OTA 卡片也可能无独立撤销入口。这是代码接线缺口，不是单纯“没有测过”。

收尾：两条恢复投影都显式携带经确认的 external_id；增加 store 恢复→组件按钮→精确调用 API 的测试。不要靠剥 UI id 前缀猜身份。

### A2 — P2：持久撤销不在统一审计时间线中

证据：`broker_approval.rs` 的 revoke_call/revoke_with_reason 写 `ota_approval_revocations`；`session_events.rs:200` 的 audit_timeline 只合并 session_events 和 run_events，没有读取撤销表，也未看到撤销路径原子追加对应事件。

影响：撤销确实生效且有表记录，但用户从统一审计时间线看不到完整撤销因果；不能表述为“审批到撤销全链已经统一可回放”。

收尾：事务内记录或在审计查询中明确投影撤销，保留精确调用、原因、时间和重复请求语义；补查询/回放测试。

### A3 — P2：撤销成功状态只存在组件内存

证据：`toolRuns.tsx` 用 useState 保存 done；ToolRunGroup 收起时卸载子行；没有撤销状态查询或 durable tool 投影。

影响：收起再展开会回到“可撤销”，重载也不能显示持久撤销状态。后端幂等校验仍保护实际操作，这主要是状态展示与可解释性缺口，不是审批绕过。

收尾：把持久撤销结果带入工具状态投影，展示“已撤销，执行状态另行确认”，不要把撤销显示成进程已退出。

### A4 — P1 验收门槛：OTA 工具链可信度与命令兼容性未得到实证

证据：`capability_broker.rs:1166` 从环境变量/固定目录发现 jar；`is_packaging_tool`（约 1204 行）主要检查普通文件和文件名，未验证签名/固定摘要。Java argv 固定包含 `--mode ota --hap ... --out ...`。本轮未运行受支持版本的实际 jar，也未验证产物格式/签名。

判定：不能把“发现名为 packagingtool.jar 的文件”称为二进制真实性验证，也不能从固定 argv 单测推出该版本 CLI 可工作。本轮没有证据断言 CLI 一定错误，列为必须验证的发布门槛。

收尾：记录受支持工具版本与来源、实际帮助/退出码、脱敏命令轨迹、有效产物和负例。若产品威胁模型要求可执行文件真实性，补独立校验；否则明确可信管理员配置边界。

### A5 — P1 发布边界：默认安全沙箱与“语义安全修改”仍未完成

证据：cmd_tools.rs 仅显式 native 配置启用原生沙箱；sandbox.rs 明确 Windows 生命周期 unavailable、原生 CPU/memory/pids 等不强制。code_mutation.rs 的 Java 分支为游离注解数量门禁，其他部分语言为 delimiter fallback。

影响：不能对外承诺“默认所有 Shell/build/test 已安全隔离”“所有语言改代码不会缺括号/留注解/类型错误”。结构句柄减少定位错误，但不能代替完整编译器和测试。

收尾：明确支持矩阵与默认策略；按目标语言补类型语义/编译验收。原生功能不支持时保持明确拒绝，不引入 Docker 作为隐含必需项。

### A6 — P2：Broker 逻辑请求身份未包含工作区

证据：`capability_broker.rs:552` 的 request_identity 接收 ctx/capability，不接收 workspace；相对本地路径进入 request_material，canonical root 未进入通用 Broker digest。

影响：若同一 tool-call 内切换根目录并操作同名相对路径，可能被误判重复。现有 OTA scope 额外绑定根目录，不能因此直接推断 OTA 可以越权；这是通用身份契约完整性风险。

收尾：显式区分逻辑目标/工作区身份/执行副本路径，并测试多根场景；随机副本路径仍不应改变幂等语义。

### A7 — P2：文档完成口径与现实漂移

- ROADMAP.md 多项已勾选是阶段契约与固定场景完成，不等于实时真机交付完成。
- HEADLESS_AGENT_DRIVER.md 同时保留早期“生产循环尚待迁入”与后续 headless 已接入的历史段落；当前桌面仍调用 begin_persisted_round，headless 已使用 run(port)。
- 演进文档头部标记 2026-09-10，却含后续更新；Broker 总览仍称自身审批字段待做，而 OTA P0—P6 已部分实现。
- 旧工具链验收有最多 32 工具，演进目标是 8—20；不能把目标或局部单测与当前生产 32 上限混为一谈。

收尾：建立“当前状态单页 + 历史日志”，一个能力单独列实现状态和验收证据，不再用不断追加 Px 小节代替主线关闭。

## 5. 百万仓库能力的准确结论

历史证据 `INDEX_SCALE_BASELINE.md`：1,000,000 文件已登记目录；首批解析 4,000，尚 deferred 996,000；冷目录+首批约 48.99 秒，单文件增量约 227 ms。百万 SCIP 引用也有独立基准。

这证明全库可达和渐进机制的数量级，不证明百万真实文件全部解析、所有调用关系准确或任意查询高召回。后台渐进解析确有生产接线（ensure_progressive_indexing），不是只有基准桩；但仍需报告它在真实仓何时收敛到完成以及期间的用户查询质量。

物理分片不必为了打勾先做，应该以真实 SLO 触发。当前优先级是正确的 coverage/staleness、混合仓召回率和外部批量改动恢复。

## 6. 发布/资源维护

- 本机盘点：`src-tauri/target` 约 36 GB、node_modules 约 509 MB、原 dist 约 8.1 MB。target 是生成物占用，不应表述为源码体积，也不能未经分类全部清理。此轮未删除文件。
- 本轮 Web build 正常更新既有 dist；没有生成新的百万测试仓或安装包。后续清理先 dry-run，保留必要缓存与用户产物。
- release.yml 配置了 Tauri 更新签名，不足以证明平台代码签名、公证、SBOM、provenance、新 VM 验收全覆盖。需按每个发行目标列实际产物与校验记录。
- 真实评测 workflow 是手动触发，并在上传产物后有最终退出码门禁；不能仅因中间 continue-on-error 判成无门禁，也不能因 workflow 文件存在推断它已成功跑过。

## 7. 建议收尾顺序（不是已执行承诺）

1. **一致性修复包**：A1/A2/A3 + 文档状态归一。验收必须覆盖实时、恢复、切会话、折叠重挂载、撤销错误、审计回放。
2. **OTA/设备真实纵向验收**：固定 DevEco/SDK/HDC 版本；一条 happy path 加停止、超时、重启、旧包、输入变更、未知结果负例。先验证工具本身能完成正确产物，再继续扩展签名/发布。
3. **真正未完成的主线实现**：桌面 IO port、Java 类型语义、通用 Broker 审批/身份/影响契约、原生沙箱支持矩阵。按可独立验收的能力交付，不继续仅以小节编号表示进度。
4. **真实 Agent 与大仓证据**：一个真实模型 smoke→固定回归子集；一个真实混合仓 recall/staleness 基线；随后才谈 HarmonyBench/公开 SWE 报告和分片。
5. **发行门禁**：受支持平台安装包、来源与签名、SBOM/provenance、干净机器 smoke、空间清理策略。

近期可达到的完成标准应是“限定支持范围内的可验收版本”，而不是一次性关闭 3—6 个月研究路线。

## 8. 盘点边界

这是覆盖主线各能力域的证据盘点与重点代码审查，不是整个仓库逐行安全审计。已确认的源码缺口、历史文档结果、本轮实测和未验证假设在上文分别标注；没有验证外部 CI 历史、真实设备或系统隔离强度，不将未发现问题等同于不存在问题。功能代码保持不变，本轮只新增此报告。

## 9. 后续实现进展（2026-09-15）

| 项目 | 本轮代码落实 | 仍需验证 / 剩余边界 |
| --- | --- | --- |
| A1 恢复调用身份 | 两条恢复路径共用 `toolRunFromExecutionStep`，明确携带 external_id；无身份不伪造撤销权限 | 桌面真实中断恢复、跨会话行为 |
| A2 撤销审计 | session 时间线及 UI 实际使用的 `enterprise::list_audit` 均投影持久撤销表；保留原因、调用、时间和 run 过滤 | 桌面审计页面纵向验收 |
| A3 撤销展示 | 新增只读状态 API；重挂载查询持久状态；旧调用异步返回不能污染新卡片 | 真机派发与撤销竞态；已撤销不等于进程已停止 |
| A6 工作区身份 | Broker v2 身份绑定 canonical 工作区；随机执行副本不参与身份；旧 v1 claim 保守拒绝自动重放 | 多版本混跑和真实多根工程回归；不支持旧二进制反向降级安全保证 |
| 常驻工具集 | 生产排名后上限 20，固定保留 7 个计划/澄清/发现入口；验证阶段保护原候选中目标必需的验收工具；文本提示与 schema 默认预算同步；小预算与零预算边界修复 | 同模型 A/B 成功率；不是已经完成完整程序化工具编排 |
| 超长命令输出 | 用受限行缓冲替代先读整行再截断；兼容按行 smart_decode | 跨平台实际构建编码与超长输出回归 |
| 原生沙箱输出资源 | stdout/stderr 共用采集预算，在推送事件前限额；超限仍排空管道；截断标志传回 sandbox result | 不代表 CPU/内存/PID/临时磁盘总量限制已完成 |

未关闭的代码主线仍包括：桌面 IO port 完整迁移、Java 类型语义变更保护（已推进到单文件 javac 差分门禁，JDT LS 语义联动与 Maven/Gradle 全工程验证仍缺，见第 15 节）、原生系统资源限制/Windows 生命周期、非 OTA Broker 审批影响契约和签名发布能力。不得将这些实现缺口改标成“只剩真实验证”。

验证记录：前端 14 文件 120 项通过，lint 与 TypeScript 检查通过；Web build 与 bundle gate 通过。后端最终库回归为 1,025 通过、9 忽略（总计 1,034）；包括真实本机子进程的双管道超限排空和非零退出码测试，但不代表系统沙箱边界验证。没有运行 Docker、真实 Provider、设备或发行安装包。

两组崩溃恢复集成 `tool_worker_crash_e2e`、`worker_crash_e2e` 各 3 项通过。非测试构建仍有既有 dead-code 警告；没有将回归通过表述为零警告或真实产品验收完成。

## 10. Java AST 写入门禁（2026-09-15）

- 锁定 `tree-sitter-java=0.23.5`，复用当前 Tree-sitter 运行时；采用 [官方 Rust grammar 接口](https://docs.rs/tree-sitter-java/0.23.5/tree_sitter_java/)。Cargo.lock 只新增该 grammar，没有升级现有依赖。
- Java 候选验证从逐行扫描升级为真实 AST；语法错误和注解声明目标错误分别比较，不能靠修好一种错误抵消新增另一种错误。
- 标准 `@Override` 必须附着方法，`@Resource` 只接受支持的类型/字段/方法声明；忽略注释和字符串，支持多行参数与标准限定名。未知限定名的自定义注解不套用标准规则；未限定注解名按标准注解处理，尚不解析同名类型遮蔽。
- `write_file`、`edit_file`、`multi_edit` 复用门禁；新增实际入口测试，确认拒绝时两个原文件均不改变。既有结构句柄连同注解删除字段/方法的回归保持通过。
- 语法树错误统计改为游标迭代，避免递归遍历深树占用调用栈。
- 库回归：1,030 通过、9 忽略（总计 1,039），含 9 项候选验证测试及新增 Java 多入口事务测试。
- 锁定依赖离线运行两组崩溃恢复集成，各 3 项通过；保留既有非测试 dead-code 警告。本批未改动前端，未重跑 UI 或真机验收。

边界：这是语法/声明形态检查，不是类型语义分析。不存在的父类、错误的 override 继承关系、删除仍在使用的 import 等仍需 JDT/Javac/工程构建验证；同错误数的语法变化也未获得语义正确性保证。没有把 Java P1 整体勾选完成。

## 11. Java 结构句柄边界保护（2026-09-15）

发现：当前结构句柄主要使用行范围。同一行存在多个声明时，删除其中一个声明对应的行可能连同邻居删除，且删除后的文件依然语法合法。因此仅验证候选 AST 不足以防止误删。

已落实：

- Java 句柄编辑前核对真实 AST 声明种类与起止行；声明前后只允许同一行的空白，禁止隐含吞并相邻内容。
- 多变量字段不能当成单字段节点删除；未覆盖完整附着注解的范围拒绝；旧句柄缺少类型证据时要求重新获取结构。
- 单句柄、批量句柄与 dry-run 都先校验边界，不靠最终语法检查猜测原始选择是否安全。
- UI 增加 boundary 状态及中英文解释，明确不会通过重新授权绕过歧义；原始错误仍保留。
- 真实工具入口回归覆盖单句柄删除、预览、批量句柄；拒绝后原文件与同一行邻居保持不变。正常独立行的带注解方法/字段编辑保持既有回归覆盖。

限制：这是保守拒绝策略，不是字节级 AST 编辑。带同行注释或额外前导文档的句柄也可能被拒绝，应读取后采用明确的精确文本事务；Java 索引尚未整体迁移到 AST，JDT/Javac 类型语义仍未完成。普通 old/new 与非句柄 start 模式不冒充此边界保证。

本批验证：后端库 1,032 通过、9 忽略（总计 1,041）；前端 14 文件 122 项通过；lint、TypeScript、Web build 与 bundle gate 通过。仍有大 chunk 警告；本批没有重跑系统沙箱、真实模型、设备或安装包验收。

## 12. Java 单节点字节级修改（2026-09-15）

- 单句柄在完整 SHA、节点身份、根目录及受控重定位校验之后，通过 Java AST 从绑定行范围选取唯一同种声明，使用 parser 的 UTF-8 字节区间拼接候选文件。
- 注解随声明进入字节区间；同行其他种类声明、前后注释与 Unicode 内容保持原样。多个同种声明占据同一范围时仍失败关闭，不猜测名称；多变量字段仍不能按单字段删除。
- 批量句柄继续使用第 11 节的整行独占门禁。没有修改句柄序列化版本，也没有取消旧句柄过期检查。
- 单节点 dry-run 提前执行候选语法检查，避免无效内容在预览时被当成可应用；预览不落盘、不消耗旧句柄。
- 新增纯 AST 字节边界测试，以及实际工具入口的预览、删除、同行字段/注释保留、旧句柄拒绝和无效预览拒绝测试。

未完成：结构索引本身仍主要是行范围；字节定位只用于可唯一匹配的 Java 单节点消费端。批量字节事务、其他语言适配和 Java 类型语义仍需实现，不能标记整条主线完成。

本批最终库回归：1,034 通过、9 忽略（总计 1,043），锁定依赖离线运行；`git diff --check` 通过。本批未修改前端，未重跑 UI、真机或发行安装包验收。

## 13. Java 批量字节事务与提交前复验（2026-09-15）

- Java 同文件 `symbol_handles` 先在同一份原文解析所有节点的字节范围，重复范围及父子重叠整体拒绝；按字节顺序拼接一次候选，替换内容与结果报告仍绑定原参数顺序。
- 保留同行邻居、CRLF 与原文字节；任一定位失败或合并候选无效都不写入。普通 `starts` 与其他语言继续使用原行块事务，不冒充字节 AST 编辑。
- `edit_file` 共用写入助手在真正写入前重新检查普通文件类型、长度与完整字节内容。外部同长度改写或删除不会被候选覆盖，也不会误触发恢复旧内容。
- 测试覆盖乱序参数、重复/父子重叠、参数数量不匹配、批量预览不写入、错误候选整体拒绝、同行字段保留及晚到外部变更。

边界：这是同文件批量候选事务；不是跨文件字节事务或文件系统原子 compare-and-swap。最后一次读取与实际写入之间仍有不合作外部进程的竞态窗口；不把提交前复验说成完整跨进程并发协议。Java 类型语义及通用跨语言字节索引仍待完成。

验证：后端库 1,037 通过、9 忽略（总计 1,046）；清理旧整行测试辅助函数后，候选门禁定向测试 11 项通过。提交前复验读取上限为原基线长度加 1 字节，避免外部突然扩大的文件导致无界读取；新增不同长度改写保持不变的测试。`git diff --check` 通过；本批未修改前端，未重跑 UI、真机、系统沙箱或安装包验收。

## 14. 多文件提交与恢复状态真实性（2026-09-15）

发现：`multi_edit` 原先直接写入每个候选，回滚忽略 `write` 错误并统一报告已恢复，还可能覆盖事务开始后的外部新内容。

- 整批所有基线先通过核验才开始写；每个文件写入前再次使用有界的完整内容复验。
- 回滚按相反顺序进行，仅覆盖仍匹配本事务候选的文件；已经等于原始基线的文件幂等跳过，后来被外部修改的文件保留并报告。
- 当前失败文件是否回到原始基线会再次核验；任一项无法恢复则返回“回滚未完成”及文件列表，不再吞错或虚报全量回滚。
- UI 增加独立中英文恢复未完成状态；错误信封给出人工核验建议。恢复未完成优先于 busy/timeout 等关键词，禁止自动重试提示。
- 测试覆盖整批预检拒绝不写入、逆序条件恢复、外部内容保留、重复恢复、错误重试策略和 UI 非成功回滚提示。

仍不承诺跨进程文件系统 CAS：不合作进程仍可能在最终检查与写入之间改写。跨文件提交也不是操作系统级原子事务，失败状态必须依据逐文件核验处理。

验证：后端库 1,039 通过、9 忽略（总计 1,048）；统一诊断建议后，恢复错误重试策略定向测试通过。前端 14 文件 124 项通过，lint、TypeScript、Web build 与 bundle gate 通过；仍有大 chunk 警告。未运行真实模型、设备、原生系统边界或安装包验收。

## 15. Java 编译器差分诊断门禁（2026-09-15）

发现：Tree-sitter 门禁只覆盖语法与注解形态。删除仍在使用的 import、改成不存在的父类、写出并不覆盖任何方法的 `@Override`，都属于“语法合法但编译不过”，此前会直接落盘。

已落实：

- 新增 `src-tauri/src/agent/tools/java_compiler.rs`：基线（当前文件内容）与候选各写进独立临时目录，按包名还原目录结构并按包名从文件路径上溯推导源码根作为 `-sourcepath`，以 `javac -proc:none -nowarn -implicit:none -Xmaxerrs 2000 -J-Duser.language=en -d <tmp>` 编译，只比较 stderr 诊断，临时目录用完即删。
- 诊断签名归一化：剥掉文件路径与行列号，保留错误消息与 `symbol/location/required/found/reason` 细节行并折叠对齐空白。既有错误的行号漂移不会被当成新增；同一签名在候选中多出现一次才算新增——因此 `/tmp/.../A.java:2` 与 `:12` 是同一签名，而 `symbol: class Bar` 是新增。
- 差分而非绝对判定：未解析的工程 classpath 会在基线与候选中同时报错并被抵消；候选无诊断时走快速路径，只编译一次。
- 入口统一：`code_mutation::validate_candidate_with_types` = 既有语法/注解门禁 + Java 编译器差分，`write_file`、`edit_file`（含 starts 批量、Java 单节点与批量句柄、dry-run 预览）、`multi_edit` 的单文件准备阶段与 LSP WorkspaceEdit 共用同一入口；拒绝时原文件不落盘。
- 无 javac 或单次编译超过 10 秒：**不阻塞写入**，但记录 `java_type_gate_unavailable` 事件并把状态标注为“未做类型校验”，不冒充已校验。
- 新增 `utils::process::output_stderr_blocking_with_timeout`：stderr 落临时文件（避免大输出写满管道缓冲死锁），超时返回 `Ok(None)` 交由调用方降级；顺带抽出 `blocking_command` 消除与 `output_blocking` 的重复。
- UI 增加独立的 `typeCheck` 状态与中英文说明，与语法门禁分开呈现：类型错误和语法错误的修复方式不同，不能共用同一条提示；英文侧同时匹配 `java compiler gate`。

边界（**不勾选** P1 Java 语义闭环）：

- 这是 javac 差分，不是 Maven/Gradle 全工程构建。未解析工程 classpath、JDT LS 语义联动、**未参与本批**的调用方仍不在覆盖内（改动文件破坏一个没被一起编辑的调用方，javac 不会去编译它，需要按引用反查受影响文件）。
- 依赖工程 classpath 才能解析的合法引用，在源码根推导失败时可能被保守拒绝；这是可解释的保守策略，不是编译语义完整保证。
- 编译器即本机或内置 JDK 版本，未做目标字节码版本与多 JDK 矩阵验证；诊断消息文本以英文锁定，避免 locale 漂移。

验证：后端库 1,051 通过、9 忽略（总计 1,060），含 10 项 `java_compiler` 单测（本机 javac 差分断言真实执行、未走跳过分支）、1 项真实写入入口“拒绝且原文件不变”测试、1 项缺程序降级测试；两组崩溃恢复集成各 3 项通过。前端 14 文件 125 项通过，lint、TypeScript、Web build 与 bundle gate 通过（仍有大 chunk 警告）。`git diff --check` 通过。本批未运行真实模型、真实设备、原生系统边界或安装包验收，未重跑百万文件基准。

补充（同批联编，同日）：`multi_edit` 原先逐文件跑类型门禁，看不到批内跨文件破坏。现在改为每文件只跑语法/注解门禁，整批准备完后由 `validate_batch_types` 把本批 Java 候选作为一次 javac 调用的显式输入联编一次——既把 N 个文件的 2N 次编译降为 2 次，又能拦住“本批改了 A 的方法、B 还在调用”。诊断签名新增相对文件位置（如 `a/B.java`），绝对临时路径仍被剥离，两侧才逐条可比；影子源码根排在 `-sourcepath` 首位，本批未显式给出的同类文件优先解析到候选版本。

边界不变且更明确：**未参与本批**的调用方仍不在覆盖内——javac 不会主动编译没被传入、也没被隐式引用的文件，要覆盖它必须按引用反查受影响文件后一并编译，属后续工作。

验证：后端库 1,055 通过、9 忽略（总计 1,064），新增 3 项 `java_compiler` 批内联编单测（批内破坏被拦、同批改对不误拒、空批次不触发编译）与 1 项 `multi_edit` 真实入口测试（先自检 javac 是否可用，可用时断言批内跨文件破坏被拒且两个文件都不变，不可用才断言降级放行，两条路径不互相冒充）；两组崩溃恢复集成各 3 项通过。本批未改前端，未重跑 UI、真机、系统沙箱或安装包验收。

补充（覆盖未编辑的调用方，同日）：按上一段的边界继续推进，把「未参与本批」的调用方也纳入编译。

- `code_mutation::affected_java_sources` 用 Tree-sitter 取出改动前文件声明的类型/方法/构造器/字段名，在同源码根下按声明名做文本预筛，命中才并入编译；预算为最多并入 20 个文件、最多扫 400 个文件、单文件不超过 1MB，跳过 `.` 目录与 `should_skip_dir` 认定的构建产物目录。
- 预筛不追求精确：差分校验对「多纳入无关文件」是安全的（两侧同集合编译，无关文件既有错误互相抵消，代价只是更慢），因此宁可多纳入也不漏真正的调用方。这也意味着**纳入更多文件只影响耗时，不影响判定正确性**。
- 受影响文件必须两侧同集合：任一侧带上它们后编译超时才整体退回只编本批（保住既有保证），并记 `java_type_gate_affected_skipped` 事件、在拒绝文案里如实写「调用方未覆盖」，不把降级说成已覆盖。并入成功时文案写明「并编译了 N 个可能引用改动的未编辑文件」。
- 反查只在源码根为绝对路径且不是文件系统根时进行，避免无边界扫描；相对路径或取不到源码根时按「未覆盖调用方」处理。

仍需外部条件的部分：按引用反查用的是声明名文本预筛而非精确语义图，跨包同名符号、被反射/配置引用、以及超出 20 个文件预算的调用方仍可能漏；真正的「受影响文件全集」需要语义索引（SCIP/LSP）的 freshness 保证。

验证：后端库 1,057 通过、9 忽略（总计 1,066），新增 1 项反查单测（只并入提到声明名的文件、相对路径与无声明名都不扫描）与 1 项真实入口测试（`edit_file` 只改 A，未编辑的调用方 B 仍调用被删方法 → 拒绝且两个文件都不变；无 javac 时断言降级路径）；两组崩溃恢复集成各 3 项通过。本批未改前端，未重跑 UI、真机、系统沙箱或安装包验收。

## 16. 宿主能力审批凭据泛化（阶段 1，2026-09-15）

发现：执行链路本身已经通用（claim/fencing/幂等/`audit_subject` 影响面），真正 OTA 专属的是**审批凭据**——`broker_approval.rs` 里 `tool_name='ota_pack'` 硬编码 4 处，scope 固定为 `OtaScope`，撤销表叫 `ota_approval_revocations`。非 OTA 能力只有一次 UI 弹窗：没有凭据、没有撤销、执行期不复核。

本阶段只做**机制通用化**，签发与执行期复核的接线留在阶段 2（因此现在还不能说“非 OTA 能力已受凭据保护”）：

- 凭据升级为 v4 且与能力无关：新增 `ApprovalScope::{Ota, Request}`（按 `kind` 标签区分），`record_capability_approval` / `verify_capability_approval` 按 `tool` 参数化，工具名不再硬编码。OTA 保留 `verify_ota_approval` 包装（要求 scope 确为 Ota）与全部调用点，行为不变。
- **撤销判定改为自描述**：可撤销 = 该 `tool_runs` 仍处活动状态**且签发过持久凭据**（`EXISTS` run_events 同 `tool_call_id` 的凭据事件）。不再维护工具白名单，OTA 自动被覆盖；没有凭据的活动调用不可撤销，并给出明确原因。
- 迁移 084 只 `ALTER TABLE ADD COLUMN tool`（不重建表）：旧二进制仍能按 `call_id` 读到撤销记录，降级不会绕过已撤销的调用。表名保留历史名（首个使用方是 OTA），schema 已通用，此处如实标注名称与语义的偏差。
- `guards.rs` 改调通用签发入口（OTA 传 `ApprovalScope::Ota`），证明通用 API 能承载既有链路。
- 旧 v3 及更早凭据一律失败关闭，要求重新审批（与既有“旧版本保守拒绝”口径一致；重启/升级后 `process_epoch` 本就使其失效）。

边界与未做：

- **签发范围没有扩大**：`pre_approval` 仍只为 `ota_pack` 冻结 scope 并签发，非 OTA 能力的审批行为与之前完全一致；执行期也仍只有 OTA 复验凭据。泛化的是能力（谁都能签），不是覆盖范围。
- 请求型作用域（`Request { request_key, workspace }`）已在单测中验证可签发/复核，但生产路径尚无调用方。
- 尚无按能力区分「必须持有凭据」的策略层，也还没有把 `tool` 写进审计投影（时间线仍只显示 `tool_call_id`/reason）。

验证：后端库 1,059 通过、9 忽略（总计 1,068），新增 2 项凭据单测（通用请求型凭据按工具名隔离且不被 OTA 复核消费；撤销资格要求活动且有凭据，并落 `tool` 列）；既有 13 项审批/撤销测试全部保持通过（含跨连接可见性、生命周期失效、幂等复用拒绝）。迁移计数同步到 84 后 `scripts/check-docs.py` 通过。两组崩溃恢复集成各 3 项通过。本批未改前端，未重跑 UI、真机、系统沙箱或安装包验收。

补充（阶段 2：执行期复核，同日）：把凭据接到签发与执行的接线，让变更类能力真正受「可撤销 + 停止即失效」约束。

- `capability_broker::RECEIPT_REQUIRED_TOOLS` 是**显式契约清单**（deploy、deploy_all、uninstall_app、clear_app_data、grant_permission、create_emulator、start_emulator、stop_app、device_file、set_wifi_state、set_airplane_mode、screen_record、record_ui）。它是显式枚举而非自动推导：工具名与能力的对应关系分散在各工具实现里，没有单一映射表可反查。测试断言每个名字都在 `TOOL_SPECS` 注册、且对应能力不在只读白名单，避免工具改名后 fail-closed 复核误伤正常工具。
- `execute_host_capability`：非 OTA、能力不可重放、调用带 `tool_call_id` 且有 App 上下文时，派发进程前必须复核到有效凭据。工具名以 `tool_runs` 持久台账为准（不接受调用方自报），复核涵盖撤销状态、生命周期（进程 epoch/停止代次/30 分钟 TTL）与请求身份；拒绝时写 `host_capability.rejected`（`reason=missing_or_invalid_approval_receipt` 并带具体原因）。
- `guards::pre_approval`：契约内工具在**弹窗批准**与**免弹窗放行**两条路径都签发凭据，决定来源分别为 `explicitly_approved` 与 `auto_approved`。auto 表示「跳过弹窗」是用户配置的策略（allow_all/项目白名单/项目信任/会话记忆），不是绕过审批；复核接受两种来源，`rejected` 及其它非法来源仍失败关闭。
- 边界不变式：用户直接在界面发起的设备操作没有 `tool_call_id`，不属于 Agent 权限边界，不受此约束；离线/无 App 上下文（测试）同样不强制。

本次边界（如实标注）：

- 请求型作用域绑定的是 run + 工具调用 + 工具名 + 原始参数的幂等键（签发时与台账逐字核对），**不做输入内容摘要**——内容绑定仍是 OTA 专属（文件摘要 + 只读副本）。因此「审批后替换参数」在进程内由签发时的台账核对拦截、跨重启由凭据失效拦截，但没有 OTA 那样的二次内容比对。
- 清单是人工维护的：新增变更类能力若未同步，就仍在既有权限弹窗保护之下、不在本契约内。
- **接线缺少端到端自动化测试**：`pre_approval` 的签发分支与 `execute_host_capability` 的复核分支都需要 Tauri `AppHandle` 与真实台账，当前只有原语级单测覆盖。不把「原语已测」表述为「接线已验收」。

验证：后端库 1,061 通过、9 忽略（总计 1,070），新增「凭据契约清单与工具注册表一致」与「auto 凭据被接受、rejected/非法来源失败关闭、撤销后立即失败、台账无此调用不可凭空复核」两项测试；既有 16 项 broker_approval 测试全部通过。两组崩溃恢复集成各 3 项通过。本批未改前端，未重跑 UI、真机、系统沙箱或安装包验收。

补充（阶段 3：影响契约进审批与审计，同日）：审批弹窗此前对 OTA 之外的能力只有工具名、参数和一句简介，用户看不到「改什么、能不能撤销、影响到哪里」。

- 新增 `agent/impact.rs`：`ImpactContract { reversibility, scope, targets, note }`，可逆性分 `reversible` / `hard_to_reverse` / `irreversible`，范围分 `device` / `app_data` / `workspace` / `host`；覆盖 OTA 与 `RECEIPT_REQUIRED_TOOLS` 的 13 个变更类工具，**未覆盖的工具返回 `None`**，弹窗保持原样，不做猜测性描述。目标从参数中的已知键提取（device/bundle/path/hap_path…），去重并最多展示 3 个。
- 口径同源：影响描述同时进入 ① 审批事件 `chat-tool-approval`（前端弹窗）② `interactions::begin` 的持久 payload（跨重启恢复的待确认项与弹窗一致）③ 凭据事件 `host_capability.explicit_approval` 的 `impact` 字段（审计时间线直接读到用户当时看到的那套说法）。
- `list_pending_confirmations` 恢复待确认项时按同一函数重算（该处 `args` 已脱敏，目标值也一并脱敏，不引入新信息）。
- 前端：审批卡片新增影响区块（语气色 + 可逆性 + 范围 + 一句话后果 + 影响目标），中英文文案齐备；映射抽成纯函数 `impactDisplay`，对**后端新增而未在前端登记**的取值回退到通用文案，不把原始值直接透给用户。
- 影响描述是展示与审计口径，不参与权限判定：权限分级仍在 `permissions`，凭据与撤销仍在 `broker_approval`；参数不是合法 JSON 时按「无影响说明」处理，不因此阻断调用。

边界：`targets` 只是参数里的显式目标，不代表全部受影响对象（例如应用被卸载后其依赖的外部服务状态不在其中）；`note` 是固定文案，不含运行期推算的规模评估（如「将覆盖 3 个设备」）。

验证：后端库 1,067 通过、9 忽略（总计 1,076），新增 6 项 `impact` 单测（未覆盖工具返回 None、不可逆操作的范围与目标提取、OTA 目标键顺序与去重、序列化形状）；前端 14 文件 127 项通过（新增 2 项 `impactDisplay` 测试，含未知取值回退），lint、TypeScript、Web build 与 bundle gate 通过。本批未重跑真机、系统沙箱或安装包验收——**审批卡片的实际视觉效果未在真实界面确认**（无 GUI 验收环境），仅由类型检查与纯函数测试保证。

补充（阶段 3 收尾：把判定逻辑变成可测纯函数，同日）：上一段如实标注了「guards 签发分支与 execute 复核分支没有端到端自动化测试」。本轮把两处**判定逻辑**抽出来，让接线里最容易出错的部分有回归。

- 新增 `capability_broker::receipt_decision(tool, needs_approval)`：签发决策的唯一来源（`None` / `Auto` / `Explicit`），guards 的免弹窗与弹窗批准两条分支都改为消费它，不再各自判断 `requires_durable_receipt`。
- 新增 `capability_broker::requires_receipt_check(replay_safe, ota, has_call_id, has_app)`：执行期是否强制复核凭据的唯一判定，`execute_host_capability` 改为调用它。
- 新增回归断言：契约内工具弹窗批准→`Explicit`、免弹窗→`Auto`、契约外→`None`；`ota_pack` 只在显式确认后签发（它的作用域是文件内容摘要），并**守护这条不变式**——`permissions::requires_fresh_explicit_approval("ota_pack", {})` 必须为真，否则 OTA 会落到不签发凭据的分支、执行期内容复核必然失败；执行期复核的五个边界（只读能力/OTA/界面直接调用/无审批基础设施）逐条断言。

仍未覆盖的部分（如实标注）：钩子与执行入口的**调用点本身**需要 Tauri `AppHandle` 与真实台账，仍未做端到端自动化；本轮补的是判定逻辑的回归，不是「接线已验收」。

关于原计划的阶段 4（通用效果证据）：**本轮不实施**。可本地落地的部分只剩「执行结果台账」（能力 id、退出码、耗时、影响对象），与既有 `host_capability_claims`（started/succeeded/failed + subject）高度重复；真正有价值的效果证据（安装后的版本、设备是否出现、产物摘要）必须连真机或真实 hdc 才能产生和验证，写了也只能是不可验证的解析代码。待有设备环境时按此顺序实施：① 定义 `HostEffectEvidence` 契约并落 run 事件；② 先接 `deploy`/`install`（hdc 输出解析 + 安装后 `bm dump` 版本核对）与 `emulator.start`（hdc list 出现）；③ 负例覆盖安装失败、版本不符、设备中途掉线。

验证：后端库 1,067 通过、9 忽略（总计 1,076）；两组崩溃恢复集成各 3 项通过。本批未改前端，未重跑 UI、真机、系统沙箱或安装包验收。

## 17. 原生沙箱资源限制落点（2026-09-15）

发现：`SandboxSpec.limits` 里的限额只有 OCI 后端在用（`--cpus/--memory/--pids-limit/--tmpfs`），原生后端（macOS `sandbox-exec`、Linux bubblewrap）既不施加也不上报——审计里「原生 CPU/内存/PID 不强制」正是指这一点。

**先实测再设计**（macOS arm64，直接调 `setrlimit`/`ulimit`，不靠文档推断）：

| 限额 | 实测结果 |
| --- | --- |
| `RLIMIT_CPU` | 可设置且**真实生效**：自旋子进程 CPU 时间用尽后被信号终止（退出码 152 = 128 + SIGXCPU(24)） |
| `RLIMIT_FSIZE` / `RLIMIT_NOFILE` | 可设置 |
| `RLIMIT_AS` / `RLIMIT_DATA` | **内核不接受修改**（setrlimit 返回 EINVAL）→ macOS 无法用 rlimit 限制内存 |
| `RLIMIT_NPROC` | 可设置，但语义是**按用户全局计数**：软限设到 40 后子进程连 `fork` 都失败（宿主已有数百进程），不能表达「容器内进程数」 |

已落实：

- `ResourceLimits` 新增 `cpu_seconds`（`#[serde(default)]` = 600，`validate` 限 1–3600，旧 spec 仍可反序列化）；OCI 映射 `--ulimit=cpu=`，原生映射 `RLIMIT_CPU`。
- 新增 `src-tauri/src/agent/native_limits.rs`：`from_spec` **只映射语义等价**的限额（`cpu_seconds`、`memory_mb`→`RLIMIT_AS`）；`apply` 在 fork 之后、exec 之前用 `pre_exec` + `setrlimit` 挂到子进程。
- 平台能力用**运行时探测**而不是硬编码平台名：以当前值回写 `setrlimit`（语义空操作），失败即判定该限额不可用——macOS 上 `RLIMIT_AS` 因此被判为不可用。
- 未施加的限额**不被伪装成已限制**：`NativeLimitsReport` 分 `applied` / `skipped + 原因`，写入 `sandbox_native_limits` 事件，并附 `platform_gaps` 说明 `pids`（rlimit 语义不符）、`cpu_count`（需 cgroups）、`writable_tmp_mb`（`RLIMIT_FSIZE` 是单文件上限）为何不映射。执行器新增 `run_cmd_streaming_limited_with_native_limits` 返回三元组，把报告交给沙箱层。
- 不把「平台不支持」当成失败：命令照常执行，但审计里写明这次**没有被该项限制保护**。

边界（不勾选「原生资源限制已完成」）：

- 原生路径仍**不限制进程数、CPU 配额、临时磁盘总量**；内存限制在 macOS 实测不可用、Windows 未实现，**只有 Linux 上有望生效且本机无法验证**。
- `SandboxRunResult` 未新增字段，报告只进 run 事件；UI/工具输出尚未展示这份覆盖情况。
- 宿主直跑（默认路径）不经过这里，仍然没有资源限制。

验证：后端库 1,072 通过、9 忽略（总计 1,081），新增 5 项：真实子进程 CPU 限额终止、地址空间限额按平台能力分支断言（本机走 skipped 分支且必须带原因）、报告不隐藏未施加项、映射边界与平台缺口说明、无限额时不包装命令；既有沙箱测试全部保持通过。本批未运行 Docker/OCI、真机、系统沙箱边界或安装包验收。

补充（宿主直跑路径接资源限制，同日）：上一段的边界里写明「宿主直跑（默认路径）不经过这里，仍然没有资源限制」，本轮补上。

- 新增环境变量 `HARMONY_HOST_DIRECT_CPU_SECONDS`（1–3600）与 `HARMONY_HOST_DIRECT_MEMORY_MB`（64–65536），由 `native_limits::host_direct_limits_from_env` 解析。**未设置 = 不限制**：直跑是显式兼容模式，默认给每条命令套上限会打断正常构建（一次 hvigor 构建就可能烧掉数百秒 CPU），必须由使用者显式开启。
- **写错即失败关闭**：变量存在但取值非法时命令直接失败并给出提示，不把写错的限额当成「没配置」而静默不限制（与 `HARMONY_SANDBOX_BACKEND` 同口径）。
- 执行器新增 `run_cmd_streaming_env_with_native_limits`（env 变体 + 限额 + 报告三元组），`run_command` 的宿主直跑两条分支（shell 语法路径与直接执行路径）统一走它，在既有的「未受沙箱隔离」风险提示下追加一行资源限制实况（含未施加项与原因），模型与用户都能看到这次到底限了什么。
- 内存限额在 macOS 不可用这一事实同样会出现在这一行里（走 `skipped` + 原因），不会被悄悄忽略。

边界：这是**使用者自选的兜底**，不改变「宿主直跑不是安全边界」的定性——进程数/CPU 配额/临时磁盘总量在直跑路径同样不限制，隔离能力仍只在 OCI/原生 sandbox 路径；限额只覆盖 `run_command`（其它工具自带的执行路径未接）。

验证：后端库 1,074 通过、9 忽略（总计 1,083），新增 2 项：① 配置解析（未设置/空值=不限制，非法值失败关闭）② **真实调用执行器入口**的 CPU 限额终止（ToolCtx::empty + 自旋命令，断言报告 `applied=[("cpu_seconds",1)]` 且子进程被信号终止）；两组崩溃恢复集成各 3 项通过。本批未改前端，未运行 Docker/OCI、真机或安装包验收。

## 18. dead-code 警告清零与 clippy 门禁现状（2026-09-15）

前几批一直在文档里保留「非测试构建仍有既有 dead-code 警告」的说明。本轮把警告清到 0：

- 4 条 dead-code 警告全部是**只被测试使用**的辅助项，处理方式是按语义标注为测试用途，而不是删掉它们（删了会丢测试覆盖）：
  `KernelExecutorState::{new, completed_rounds, tool_attempts}`（生产经 `KernelIoRunLoop::new`/checkpoint 恢复，字段经序列化暴露）、`symbol_index::promote_deferred_batch_at`（生产走带取消判定的 `_if` 变体）、`java_compiler::{check, check_batch}`（生产经 `validate_candidate_with_types`/`validate_batch_types` 并入受影响文件）。
- 顺带修掉本批自身引入的 clippy 噪声：`exec_ctx` 的内部执行器把「日志/环境变量/输出预算/资源限制」收拢为 `StreamRunOptions`（参数从 9 降到 6），两个 `*_with_native_limits` 包装去掉调用方从未使用的 `log_file`（8→7）；`sandbox` 去掉一处冗余重绑定；`native_limits` 的跨平台 rlimit 常量转换加显式 `allow`（macOS 是 `c_int`、Linux 是 `u32`，不能按单一平台删掉转换）。

验证：非测试 `cargo check --lib` 与 `cargo test --no-run` 均为 **0 警告**；后端库 1,074 通过、9 忽略（总计 1,083）；两组崩溃恢复集成各 3 项通过。

**同日发现的独立问题（不属于本批引入，需单独决策）**：CI 的 clippy 基线门禁 `scripts/check-warnings.py --baseline 44` 当前失败——唯一告警 **100/44**。按 (lint, 文件:行) 逐条与本次会话新增行比对，**落在本会话新增行上的告警为 0**，即这 100 条全部是既有技术债：结构类 61 条（too_many_arguments 44 + type_complexity 17，基线当时是 31+13）、机械类约 39 条（bool_assert_comparison 14、cloned_ref_to_slice_refs 12、manual_inspect 4、unnecessary_map_or 3 等，多为测试代码）。门禁设定「机械类新增立即阻断、结构类保留为基线」，因此要么收敛机械类并重新设定基线，要么只更新基线——两者都会改动质量门禁口径，需要明确决策后再动，本批不改。
