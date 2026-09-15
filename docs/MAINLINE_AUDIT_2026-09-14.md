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

未关闭的代码主线仍包括：桌面 IO port 完整迁移、Java 类型语义变更保护、原生系统资源限制/Windows 生命周期、非 OTA Broker 审批影响契约和签名发布能力。不得将这些实现缺口改标成“只剩真实验证”。

验证记录：前端 14 文件 120 项通过，lint 与 TypeScript 检查通过；Web build 与 bundle gate 通过。后端最终库回归为 1,025 通过、9 忽略（总计 1,034）；包括真实本机子进程的双管道超限排空和非零退出码测试，但不代表系统沙箱边界验证。没有运行 Docker、真实 Provider、设备或发行安装包。

两组崩溃恢复集成 `tool_worker_crash_e2e`、`worker_crash_e2e` 各 3 项通过。非测试构建仍有既有 dead-code 警告；没有将回归通过表述为零警告或真实产品验收完成。
