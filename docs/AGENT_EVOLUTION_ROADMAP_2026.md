# HarmonyAgent Agent 能力演进路线（2026）

> 状态：讨论稿，不代表已经承诺实施  
> 调研日期：2026-09-03  
> 基线版本：`main` / `v2.1.1` / `8d7443e`  
> 目标：把外部评价转化为可验证的产品与工程路线，而不是继续堆叠功能清单。

## 0. 当前进度快照（2026-09-05）

下面按“已落地并验证 / 部分落地 / 需外部基础设施”三类标注当前状态；每类的具体证据见对应章节与 [INDEX_SCALE_BASELINE.md](./INDEX_SCALE_BASELINE.md)。这里不把“单测通过”冒充“产品能力完成”。

**已落地并通过测试/基准验证**

- 路线 B 核心检索：全库 SQLite 目录、keyset 游标查询、原生 watcher 与 Git diff 修复；`MAX_FILES=4000` 降级为首批解析预算，其余以 `deferred` 状态入目录。
- SCIP 精确引用：流式导入官方 protobuf、独立精确引用层、文件指纹失效、跨进程导入锁、外部覆盖检测、原子代次切换；`ForwardDefinition` 前向声明按定义位置处理。
- 热点符号关系：单次 500 条预算 + `relations_cursor` 统一 keyset 分页；关系查询 P50/P95 基准显示 5k→1M 引用第一页 P50 稳定约 3.6 ms。
- 统一检索入口：`repo_query` 按 `path/symbol/concept` 自动分流并标注 `source_layer`，`impact` 模式返回精确图反向依赖 + 主流约定的候选测试文件；`search_tools` 按 query 发现工具（`detail=name|summary`）。
- 沙箱命令接线：`select_sandbox_target`/`resolve_sandbox_target` 按 `HARMONY_SANDBOX_BACKEND`/`HARMONY_SANDBOX_IMAGE` 环境配置 fail-closed 选择执行目标，`run_command` 已接入 `OciBackend::run`（显式配置时容器内执行），宿主直跑持续显示风险标注（`agent::sandbox`）。
- 审批审计：`resolve_tool_approval` 决议写入 `session_events`（`ToolApproval` 事件），`audit_timeline` 把 `session_events` 与 `run_events`（沙箱升级）合并为一条按时间排序的统一审计时间线。
- Host Capability Broker 原型：`HostCapability` 类型化窄能力（hdc 连接/断开/列表、install、deploy）+ `validate` 拒绝 shell 元字符/绝对路径/`..`/非 `.hap`（`agent::capability_broker`）。
- headless eval harness：task schema v1 + 安全校验、`manifest`/`report`/`trajectory` 数据契约、`command grader`、补丁采集/应用、工作树准备、产物收集/导出（按声明 glob 复制到 `artifacts/`）、`run_trial` 编排（`AgentDriver` 可注入 trait，桩端到端验证），trajectory 复用 `session_events` 事件源（`agent::eval_task`/`eval_report`/`eval_trajectory`/`eval_grader`/`eval_patch`/`eval_workspace`/`eval_runner`，见 [AGENT_EVAL_HARNESS.md](./AGENT_EVAL_HARNESS.md)）。
- 目标驱动执行闭环：自定义输入编译为 `GoalContract`；计划模式支持生成、编辑和批准，最终计划持久化到 Durable Run、投影为可恢复步骤，并在中断恢复后继续作为每轮执行锚点。
- 文案与事实基线：`sandbox_exec` 改称“临时副本试运行”、`SECURITY_BOUNDARY.md`、README 下载/平台限制（已修正 Linux 过度承诺）、10k/100k/1M 基准生成器。

**部分落地（契约/探测/语法层就绪，接线或验收待完成）**

- `SandboxBackend`/`SandboxSpec`、可选 Docker/Podman 运行时探测、fail-closed OCI argv、超时取消与审计事件、命令接线均已就绪；产品默认路线已改为无 Docker 依赖的平台原生轻量后端，OCI 只保留为外部 CI 适配器。
- Tree-sitter/ArkTS 容错 AST 层与依赖/影响图——物理分片待真实仓 SLO 触发。
- ArkTS LSP 语义层与 `repo_query` 路由/影响面——依赖图重排的统一 planner 待完成。
- 结构化代码修改——`symbol_handle` v3 已绑定 file hash、稳定/位置 node ID、原节点内容摘要、expected kind、精确节点范围和 parent range；单节点及同文件多节点编辑直接按句柄范围原子修改，连续注解随声明进入同一节点，旧 v1/v2 句柄兼容。默认文件漂移失败关闭；显式 `allow_relocate=true` 时仅允许目标内容与身份不变、候选唯一的受控重定位。Rust/Dart 轻量 adapter 已覆盖受限可见性函数、impl/trait 方法及 Flutter 常见 class/mixin/extension、构造器/getter/函数，并进入持久增量索引。统一候选门禁已覆盖 TS/JS/ArkTS Tree-sitter 错误增量与 Java `@Override`/`@Resource` 游离注解增量检查；普通 `start/starts` 模式仍以括号/缩进扫描为 fallback，Java import/override 类型语义与 JDT/Javac 诊断仍待完成。
- headless eval harness——数据契约/grader/补丁/工作树/编排与 builtin driver 已落地；当前 builtin 支持 OpenAI-compatible 流式 Provider 回合（SSE 字节级行缓冲、停滞治理状态机、流式帧累加器组装严格 KernelTurn，停滞/提前关闭/坏帧/超限失败关闭）、受限文件工具和 `--driver builtin`；无 secret 请求规划、成本账本、acceptance stop gate 与 Provider 重试/取消/截止时间控制已进入 UI/headless 共用 Agent Kernel，run-config 单请求上限和独立工具调用预算已落地，headless 工具执行已接入与 UI 相同的 `TOOL_POLICY` 退避 + `retryable_for` 谓词（重试次数入 trial 私有 `tool_runs`；血缘式恢复编排为桌面会话特性，headless 无父运行血缘，按设计不接入），桌面 UI 流循环已切换到共用 `KernelStreamGovernor` + `KERNEL_STREAM_*` 常量。**Phase 4 消息历史/tool loop 抽取已完成**：KernelLoopGovernor（工具循环检测）、KernelRoundRouter（轮级路由）、KernelHistoryAssembler（消息组装）全部进入 UI/headless 共用内核；单轮路由进一步收敛为 `KernelRoundDecision` 的“注记 + 唯一主控制”，不再由两个 adapter 分别解释动作数组；reasoning 流回放与续写指令也已统一。headless 的多个停止布尔值已收敛为 `KernelRunState` 的唯一主终止原因，最终事件可直接审计且不会叠加错误的步数耗尽归因；UI/headless 进一步共用 `KernelExecutorState` 作为 router/governor/termination 的跨轮状态所有者，并在每次 Provider 请求前统一 deadline/cancel 优先级与剩余墙钟预算；回合数、全部模型工具调用尝试与停止验收补救次数也由 executor 统一记账，headless 固定硬上限的计数/裁决/归因已原子化。`KernelExecutorSnapshot` 以同一结构进入桌面 Durable Run 与 headless trajectory，包含具名轮级计数和稳定终止分类；final snapshot 强制终止原因非空，未终止/未跑满会失败关闭；acceptance gate 已退回纯契约/证据报告职责，补救状态只剩 executor 一个真源；空轮耗尽和最终工具循环熔断也在裁决处自动锁定精确终止原因；executor 终态在 Provider 边界保持吸收性，后到信号不能覆盖首因；安全裁决、回合计数和剩余墙钟预算已合并为原子 `begin_round`，adapter 不再存在双调用接缝；headless 的终态验收裁决也已原子锁定 Accepted/Exhausted 归因，桌面端仍保留后置 ship/review 门；固定与动态工具预算现共用原子 `begin_tool_attempt`，同时完成计数、预算裁决和循环观察，权限拒绝也不能绕过循环熔断，终态后不会再增加尝试计数；桌面动态预算 Halt 也在 executor 内精确归因为 `max_tool_calls_exceeded`，不再被笼统 governance 原因覆盖；桌面二阶段完成复核现区分 `model_accepted` 与 `completion_review_exhausted`，未确认完成的任务不会再产生成功终止快照；headless 外层 `for` 步数上限已删除，固定回合上限由 `begin_round(Some(limit))` 在 Provider 边界原子裁决并写入最终快照；成本超限也由 executor 的吸收态安全点归因，生产 adapter 已无直接终止写入。Rust lib 共 942 项（934 通过、8 项按环境条件忽略）。详见 [HEADLESS_AGENT_DRIVER.md](./HEADLESS_AGENT_DRIVER.md) §12。

**需外部基础设施（本仓库环境无法完成，按任务分别需真机/真实模型/官方 harness/签名证书；官方 SWE-bench 复现可在独立 CI 使用容器）**

- 沙箱端到端验证 + 恶意脚本逃逸套件（需可运行容器运行时）。
- Host Capability Broker 真实执行接入 `device_tools`/`build_tools`（需真机/模拟器验证）。
- 审批与沙箱升级的单一审计链合并（`session_events` 与 `runtime` 事件日志统一，需跨模块治理改造）。
- 真实 headless `AgentDriver`（`stream_chat` 12k 行 headless 抽取）+ `eval run` CLI + CI artifact（需真实模型端到端验证）。
- SWE-bench Verified 25/100、SWE-Explore、HarmonyBench v0、真实模型回归（需真实模型与官方数据集/harness）。
- file/line Recall@5/20（需真实语料与相关性标注，合成语料无意义）。
- Release 签名、SBOM、provenance、新 VM smoke test（需签名证书与 CI 环境）。

## 1. 先给结论

外部评价有价值，但四条里只有两条半成立：

| 外部评价 | 核查结论 | 当前证据 | 应对方式 |
| --- | --- | --- | --- |
| 没有内置沙箱，Shell 直接跑本机 | **基本成立** | `run_command` 限制了工作目录、危险模式和审批，但子进程仍拥有宿主用户权限；`sandbox_exec` 是复制到临时目录后执行，不是 OS 级安全边界 | 最高优先级：引入真正的执行沙箱和 Host Capability Broker |
| 百万级仓库索引与增量解析弱 | **部分成立** | 已有持久化、文件指纹和变化文件增量重扫；但每次同步仍需 walk/stat，符号索引最多 4,000 文件，内容检索只扫前 400 个文件，且主要是行级规则 | 高优先级：事件驱动、分片、持久化的混合代码索引 |
| 没有官方二进制，需要本地 Python 部署 | **不成立** | 当前是 Rust + Tauri 桌面应用；GitHub Releases 已发布 v2.1.1，发布流水线生成 Windows `.exe/.msi` 和 macOS `.dmg/.app.tar.gz` | 修正文案并提高发布可信度；补 Linux、签名/公证、SBOM 与安装验证 |
| SWE-bench 验证少 | **成立** | 已有 26 个确定性固定场景和 CI 回退门禁，但主要验证生产内核中的规则/状态机，不调用真实模型，也不是公开 SWE-bench 运行 | 建立真实 Agent Eval Harness，先小样本可复现，再全量公开 |

因此，最适合 HarmonyAgent 的演进主线不是“做一个更通用的 Trae/DSH”，而是：

> **以安全执行和可复现评测为底座，做对 HarmonyOS 工程、构建、设备和 SDK 最懂的本地 Agent；同时让其通用软件工程能力达到可公开比较的水平。**

未来 3—6 个月的资源建议：

- 35%：执行沙箱与权限边界；
- 30%：大仓代码理解与上下文检索；
- 25%：真实模型评测、数据闭环与可复现发布；
- 10%：Agent loop、工具发现和多 Agent 的定向优化。

在这三条主线稳定前，不建议继续以“工具数量”作为核心进展指标。

## 2. 当前项目并不是从零开始

HarmonyAgent 已有不少同类项目需要后补的基础设施：

- Rust Agent loop、持久任务、DAG、Worker 租约、fencing、崩溃恢复和副作用验证；
- 工作区路径校验、工具权限等级、人工审批、危险命令拒绝和审计；
- 工具专用 OS 线程、panic 隔离、进程树清理和输出上限；
- 会话压缩、任务快照、失败反思、子 Agent、工具排序和能力包；
- ArkTS LSP、HarmonyOS SDK/API 索引、hvigor/ohpm/hdc/真机诊断闭环；
- 版本化固定评测、故障注入 E2E 和 CI 基线回退门禁；
- Windows/macOS 的 Tauri 安装包与自动更新发布链路。

这意味着下一阶段应补“强边界”和“强证据”，而不是重写 Agent 内核。

### 2.1 三个容易被文案掩盖的事实

1. `sandbox_exec` 目前更准确的名称是“临时副本试运行”。本轮已收紧为 `simulate` 必须提供 `source`，但改变 `cwd` 仍不能阻止进程读取用户目录、访问网络或调用宿主上的其他程序。
2. 符号索引已实现“变化文件只重解析”，所以不能说完全没有增量索引；但候选文件发现仍是全目录遍历，硬上限也使其无法证明百万级仓库能力。
3. 现有固定评测适合防止 Rust 内核回退，但不能证明“某模型 + HarmonyAgent harness”能自主解决真实软件问题。

## 3. 近期主流 Agent 工程给出的信号

以下不是追热点，而是可以转化为本项目工程决策的共同趋势。

### 3.1 Harness 比“再换一个模型”更值得建设

OpenAI 在 2026 年的 Harness Engineering 实践中强调：大规模 Agent 开发依赖可被 Agent 导航的仓库知识、机械执行的架构约束、快速反馈和持续清理，而不是一份巨大的说明文件。Anthropic 也一直建议采用简单、可组合的 Agent 模式，按任务复杂度增加自治程度。

对 HarmonyAgent 的含义：

- 把项目结构、工具契约、验证命令和错误修复建议变成机器可读资产；
- 把“应该遵守”升级成 lint、policy、postcondition 和测试门禁；
- 模型可以替换，但同一套执行、上下文、评测和审计协议必须稳定。

### 3.2 安全默认值正在变成产品能力

主流 Coding Agent 已把“工作区写入、工作区外审批、默认断网、按域名放行”作为产品级边界。OpenAI 的 Codex 默认运行在沙箱中并关闭网络；新的 Agent SDK 进一步把 harness 与 compute 分离，以隔离凭据、支持快照恢复和弹性扩展。

对 HarmonyAgent 的含义：

- 黑名单和审批不能替代内核强制隔离；
- 模型生成的 Shell、仓库中的恶意脚本、依赖安装和 MCP 返回内容都应视为不可信输入；
- HarmonyOS 真机、签名和 DevEco 工具链需要宿主权限，应通过窄接口 broker 提供，而不是把整个 Shell 提权。

### 3.3 上下文工程正在从“全部塞进去”变成按需发现

Anthropic 公布的工具使用实践显示，大量工具定义会明显挤占上下文并增加选错工具、错参数的概率；推荐按需发现工具、用代码编排重复调用、只把最终相关结果送回模型。HarmonyAgent 已有能力包、阶段选择和工具排序，这是很好的起点，但还可以进一步做成真正的延迟加载协议。

对 HarmonyAgent 的含义：

- 201 个工具不是护城河本身；“在正确阶段稳定选中正确工具”才是；
- 模型默认只看到 8—20 个核心工具，其余通过 `search_tools`/能力包动态展开；
- 大批量搜索、过滤、聚合在沙箱内程序化执行，避免每次工具调用都经历完整模型往返。

### 3.4 Eval 已从最终分数演进为分层诊断

Anthropic 的 Agent eval 方法强调：任务、trial、grader、trajectory、outcome 和 harness 必须分别记录；由于模型输出有随机性，需要多次 trial，且最终环境状态比 Agent 自述更可信。SWE-bench Verified 使用容器化环境和隐藏测试；更新的 SWE-bench Pro、SWE-bench Live、SWE-Explore 又分别强化了长任务、数据污染和代码定位能力。

对 HarmonyAgent 的含义：

- 既要测最终补丁是否解决问题，也要测是否找对文件、是否安全、成本多少、是否重复副作用；
- 固定内核单测、真实模型回归、公开通用基准和 HarmonyOS 专项基准不能混成一个分数；
- 任何公开成绩都必须能下载预测、轨迹、日志、模型配置和评测报告。

## 4. 建议的目标架构

```text
React/Tauri UI
      |
Agent Harness
  - goal / plan / compact / recovery / acceptance
  - tool discovery / routing / trajectory
      |
Policy & Capability Broker
  - workspace policy / approval / credential handles
  - network allowlist / audit / budget
      |------------------------------------|
Sandbox Executor                         Host Capability Broker
  - shell/build/test                      - hdc/device/emulator
  - untrusted repo scripts                - signing/keychain
  - no raw credentials                    - explicitly approved deploy
  - snapshot/diff/artifacts                - typed, narrow operations
      |
Repository Intelligence Service
  - file catalog + lexical index + AST/LSP/SCIP graph
  - incremental watcher + shard cache + retrieval planner
```

关键设计原则：

- **Brain 与 Hands 分离**：Agent 状态不依赖某一个沙箱进程，沙箱销毁后可从 checkpoint 恢复；
- **默认最小权限**：只读、工作区写、宿主访问分层，不允许静默降级到无限制宿主执行；
- **宿主能力窄化**：设备、签名、部署只暴露类型化工具，不暴露等价的任意 Shell；
- **检索先于生成**：代码定位、影响分析和验证计划是独立可评测阶段；
- **最终状态裁决**：完成与否由测试、构建、diff 和设备状态判定，不由模型口头声明判定。

## 5. 路线 A：无 Docker 依赖的默认隔离（P0）

### 5.1 先定义统一策略，而不是先绑定某个容器产品

新增 `SandboxBackend` 抽象，至少包含：

```rust
trait SandboxBackend {
    fn capabilities(&self) -> SandboxCapabilities;
    async fn prepare(&self, spec: SandboxSpec) -> Result<SandboxHandle>;
    async fn exec(&self, handle: &SandboxHandle, cmd: ExecSpec) -> Result<ExecResult>;
    async fn snapshot(&self, handle: &SandboxHandle) -> Result<SandboxSnapshot>;
    async fn destroy(&self, handle: SandboxHandle) -> Result<()>;
}
```

`SandboxSpec` 至少显式声明：

- 文件系统：只读挂载、可写工作树、临时目录、禁止访问路径；
- 网络：`none | allowlist | full`，默认 `none`；
- 环境变量：白名单注入，禁止把整个宿主环境传入；
- 资源：CPU、内存、进程数、输出、磁盘和 wall time；
- 身份：无特权用户、禁止继承宿主凭据；
- 生命周期：任务/子 Agent 独立实例、快照、销毁和审计 ID。

### 5.2 推荐落地顺序

1. **先实现本机轻量后端**：核心安装和日常工具不要求 Docker/Podman；按 macOS/Windows/Linux 封装原生目录、进程、环境和网络限制。如果某平台达不到声明能力，必须失败关闭或明确显示“不受保护”。
2. **保留 `HostDirectBackend`**：只作为显式兼容模式，启动任务时持续显示风险，不作为默认值。
3. **拆分宿主特权工具**：`hdc`、模拟器、签名、发布走 Host Capability Broker，每次只获得完成该动作所需的句柄和范围。
4. **OCI 仅作为可选适配器**：用于官方 SWE-bench 镜像复现或已有容器基础设施的 CI，不进入桌面工具的安装依赖和默认执行路径。

### 5.3 需要修正的现有能力

- 将当前 `sandbox_exec` 改名为 `workspace_clone_exec`，或让它真正调用新后端；
- `run_command` 默认进入沙箱，越过沙箱必须产生独立的 approval event；
- Shell 黑名单保留为纵深防御，但不再被描述成安全边界；
- MCP 子进程也纳入相同网络、目录、环境变量和资源策略；
- 子 Agent 必须拥有独立工作树与独立沙箱，不能只隔离对话上下文。

### 5.4 安全验收

建立 `sandbox-adversarial` 测试集，至少覆盖：

- `../`、绝对路径、符号链接、硬链接、Git worktree 和挂载点逃逸；
- 读取 SSH、云凭据、Keychain/凭据管理器和父进程环境；
- DNS、HTTP、Unix socket/Named Pipe、本机回环和端口扫描；
- fork bomb、内存洪泛、磁盘写满、无限输出和孤儿进程；
- 恶意 `package.json`/构建脚本/MCP server 的间接执行；
- approval 绑定错误 call id、重放、TOCTOU 和沙箱降级。

发布门槛：默认模式下逃逸成功数必须为 0；不能建立声明边界时必须失败关闭。

## 6. 路线 B：百万级仓库代码理解（P0/P1）

### 6.1 现有实现的准确诊断

当前 `symbol_index.rs` 已有：

- mtime 纳秒 + 文件长度指纹；
- 内存与磁盘持久缓存；
- 新增/变化文件单文件重解析；
- 删除文件的符号清理；
- 扫描阶段在锁外执行，降低多项目互相阻塞。

但它仍存在结构性上限：

- `MAX_FILES = 4000`；
- 单文件上限 512 KiB；
- `codebase_search` 和引用反查最多读取前 400 个源码文件；
- 冷却期之外仍需从根目录递归 walk/stat；
- TS 系与 ArkTS 已具备容错 AST 和声明继承关系，但仍缺少跨文件名称绑定、调用关系和跨仓依赖图；
- 查询结果无法给出索引覆盖率和“因上限漏检”的明确告警。

### 6.2 建议的四层索引

| 层 | 作用 | 建议实现 |
| --- | --- | --- |
| L0 文件目录 | 路径、语言、大小、hash、Git 状态 | `ignore` 规则 + 文件 watcher + SQLite/RocksDB；按 module/shard 存储 |
| L1 词法搜索 | 标识符、字符串、错误码、配置 | ripgrep 即时 fallback + trigram/倒排/SQLite FTS5 持久索引 |
| L2 语法索引 | 定义、引用、import、组件、路由 | Tree-sitter 增量 AST；ArkTS grammar 不完整时保留容错解析 |
| L3 精确语义 | 类型、跨文件/跨模块定义引用、诊断 | ArkTS LSP 为主；为多语言预留 SCIP importer/indexer 接口 |

Embedding 应只用于自然语言查询的召回或重排，不能替代精确符号、路径和依赖检索。

### 6.3 增量更新路径

```text
初次打开 -> 读取 manifest/Git tracked files -> 分片后台索引
文件事件 -> debounce -> 只更新受影响 shard -> 更新依赖反向边
Git 切换 -> 用 diff/name-status 计算变化 -> 校验 watcher 漏失 -> 增量修复
查询到未就绪 shard -> 即时 rg/LSP fallback -> 返回覆盖率 -> 后台提高该 shard 优先级
```

不要在每次 Agent 查询前完整 walk 百万文件。完整一致性扫描可以低优先级、空闲时运行。

### 6.4 Query Planner

Agent 不应直接猜选搜索工具。新增一个统一 `repo_query`：

- 精确路径/错误码：优先 lexical；
- 符号定义/引用：优先 LSP/SCIP，失败回退 AST/lexical；
- “哪里实现了某行为”：BM25/embedding 召回后，用符号和依赖图重排；
- 修改影响面：反向依赖图 + 测试映射 + Git 历史；
- 每个结果返回 `source_layer`、`index_revision`、`coverage`、`stale` 和可引用行范围。

Agent 的默认入口采用 [Structure-first 代码导航](./STRUCTURE_FIRST_NAVIGATION.md)：先把符号按 `entity`（类/组件/类型/状态等）和 `logic`（函数/方法）组织并分页检索，再按返回的结构行区间读取正文。二分类只用于规划，索引仍保留语言原生 kind；索引无结果或 coverage 不完整时必须走 lexical/LSP fallback。现有 `search_symbols` 已加入签名、父级、起止行、角色、稳定游标、coverage 和 staleness，以及热点符号关系的 keyset 游标分页（`relations_cursor`）；TS/TSX/JS/JSX 节点进一步标注 `tree_sitter` 来源并使用 AST 精确范围，其余语言或语法错误文件明确标注 `lightweight` fallback。

### 6.5 “全库可达”与单次读写预算

百万级支持不等于把全仓文件内容同时读取进模型上下文。系统应区分两个概念：

- **全库可达**：每个未被 ignore/权限策略排除的文件都有目录记录，可以按稳定路径或 `file_id` 定位；索引必须报告覆盖率，不能静默漏掉第 4,001 个文件；
- **单次预算**：一次工具调用仍限制行数、字节数、输出 token 和耗时，防止一个超大文件耗尽上下文或内存。

建议统一文件访问协议：

1. `repo_query` 返回 `file_id`、路径、大小、hash/index revision、命中行和下一页游标；
2. `read_file` 支持 `start_line + lines`、`byte_offset + byte_length`、`symbol_id` 三种窗口，响应返回 `next_cursor` 和文件版本；
3. 文本大文件按窗口读取，语法块可以在预算内自动补齐；超预算时返回摘要和继续读取位置，而不是声称已读完整文件；
4. 修改优先使用带 `expected_hash` 的锚点 patch；落盘前重新校验版本，冲突时拒绝覆盖并要求重读；
5. 大范围机械变更由受限脚本在沙箱工作树内完成，随后用 Git diff、编译和测试验证，不让模型逐文件复制全文；
6. 二进制、生成物、压缩包和超大数据文件只记录元数据，由专用解析器按需抽取，不进入通用源码全文索引。

因此，当前 `read_file` 的单次 2,000 行/字符预算和写入大小限制可以保留，但错误信息与结果协议必须明确这是“单次窗口限制”，不是“文件不可访问”。本轮已让普通读取返回 SHA-256 `file_version`、实际窗口和 `next_start`；超过 1 MiB 的文本在显式提供 `start/lines` 后使用固定内存的流式窗口，不再加载或拒绝整个文件。深页目前仍需从文件头扫描，后续通过持久化行偏移 sidecar 支持直接 seek；大文件版本目前使用 metadata revision，版本安全修改仍需强 hash/分块 patch。最终验收应包含随机抽取首部、中部、尾部文件，证明百万文件目录中任意合规文件均可寻址、分页读取和版本安全修改。

### 6.6 大仓验收指标

构造或选择 10k、100k、1M 文件三个档位，发布以下结果：

- 冷启动可查询时间、完整索引时间；
- 单文件修改后的 P50/P95 可见延迟；
- 路径/符号/自然语言查询 P50/P95（SCIP 热点关系查询 P50/P95 已测，见 [INDEX_SCALE_BASELINE.md §7](./INDEX_SCALE_BASELINE.md)；其余各层待测）；
- 文件级 Recall@5、Recall@20，行级 Recall@20；
- 峰值内存、索引磁盘占用、CPU 时间；
- 索引重启恢复时间、Git checkout 后一致性；
- 前台查询对 UI 和 Agent 首 token 延迟的影响。

建议阶段门槛：100k 文件仓库增量更新 P95 < 1 秒、常见查询 P95 < 500 ms；1M 文件仓库可以后台构建，但 10 秒内必须具备渐进式可查询能力。最终数值应以真实机器基线校准。

## 7. 路线 C：把评测从“内核单测”升级为“Agent 产品证据”（P0）

### 7.1 保留现有评测，但重新命名分层

| 层级 | 当前/新增 | 回答的问题 |
| --- | --- | --- |
| L0 单元与故障注入 | 当前已有 | 状态机、恢复、权限和工具实现是否正确？ |
| L1 确定性 Harmony 场景 | 当前已有 | 生产规则、解析器和契约是否回退？ |
| L2 真实模型回归 | 新增 | 固定模型通过真实 Agent loop 能否完成任务？ |
| L3 公开通用基准 | 新增 | 与其他 Coding Agent 在可比条件下表现如何？ |
| L4 HarmonyOS 专项基准 | 新增 | 项目的领域护城河是否真实有效？ |
| L5 线上/狗粮数据 | 新增 | 用户任务成功率、成本与安全体验是否改善？ |

不要把 L0/L1 的 100% 通过率表述为 Agent 任务成功率。

### 7.2 先建立通用 Eval Adapter

定义与 UI 解耦的 headless 入口：

```text
harmony-agent eval run \
  --task task.json \
  --workspace /repo \
  --model <pinned-model> \
  --sandbox <backend> \
  --trajectory out/trajectory.jsonl \
  --patch out/model.patch \
  --report out/report.json
```

一次 trial 必须记录：

- Agent/Harness commit、模型精确版本、推理强度、系统提示和工具 registry digest；
- 数据集版本、instance id、仓库 base commit、沙箱镜像 digest；
- 完整可公开 trajectory（敏感推理按供应商政策处理）、工具参数/结果摘要和时间线；
- 输入/输出/缓存 token、费用、wall time、工具调用数和重试数；
- 最终 patch、测试输出、grader 结果和失败分类。

### 7.3 公开基准顺序

1. **SWE-bench Verified smoke（25 题）**：固定题目与官方 revision；官方容器 grader 只在独立 CI 适配器中复现，不成为 HarmonyAgent 本体依赖，目标是可复现，不追榜。
2. **SWE-bench Verified 100 题固定子集**：用于每周 harness A/B；同模型、同预算，至少 3 个 trial 或报告置信区间。
3. **SWE-Explore**：直接测文件/行定位，专门驱动大仓检索路线。
4. **SWE-bench Pro public 或 SWE-bench Live**：降低旧数据污染和 Verified 天花板问题。
5. **Verified 全量 500**：月度/里程碑执行，发布预测、日志和官方 harness 报告；若官方榜单停止接收，不宣称“官方排名”。

### 7.4 建立 HarmonyBench

SWE-bench 主要验证通用仓库修复，不能覆盖 HarmonyAgent 的核心价值。建议从真实 issue、构建失败和设备故障中脱敏形成至少 100 个任务：

- ArkTS 编译/类型/API Level 兼容：30；
- 多模块依赖和跨模块修改：20；
- UI 行为/截图/状态断言：15；
- hvigor/ohpm/签名配置：15；
- faultlog/hilog/性能与真机诊断：15；
- 安全拒绝、审批和恢复：5。

每题至少包含 base revision、任务描述、隐藏测试或确定性 outcome grader、允许的设备/SDK fixture、预期副作用和失败分类。先建私有 holdout，再选择可公开子集。

### 7.5 核心指标

- 结果：resolved rate、FAIL_TO_PASS、PASS_TO_PASS、build/deploy success；
- 检索：首个相关文件耗时、file/line Recall@k、无关文件读取量；
- 效率：成本/成功任务、token/成功任务、wall time、工具调用数；
- 稳定：多 trial 方差、flake rate、恢复成功率、重复副作用率；
- 安全：沙箱逃逸、越权尝试、误审批、凭据泄漏、网络策略违反；
- 体验：人工介入次数、无效确认次数、diff 接受率、回滚率。

## 8. 路线 D：Agent loop 与工具系统（P1）

### 8.1 从能力包升级到延迟加载工具协议

现有 phase-aware 选择和 tool ranking 应保留，再补：

- 常驻核心工具不超过 8—20 个；
- `search_tools(query, detail=name|summary|schema)` 动态发现其余工具；
- 工具 schema 只在选中后进入上下文；
- 为高误用工具维护正/反例，而不仅是描述文本；
- 把 wrong-tool、invalid-args、tool-not-found 分开统计并进入 eval。

先用同模型 A/B 验证工具发现是否提高成功率或降低成本，再决定是否推广到全部 Provider。

### 8.2 程序化工具编排

在真正沙箱内提供受限脚本运行器，让模型可以：

- 并行读取/过滤多个检索结果；
- 聚合日志、测试结果和依赖数据；
- 只把最后的结构化摘要送回模型。

该能力不应获得 Host Capability Broker 权限，也不能直接读取凭据。

### 8.3 验证驱动循环

把当前 goal contract/postcondition 扩展为统一状态机：

```text
Explore -> Hypothesis -> Minimal Edit -> Targeted Verify
       -> Broader Verify -> Diff Review -> Outcome Acceptance
```

要求 Agent 在编辑前形成可证伪假设；失败后优先获取新证据，不允许无证据重复相同工具调用。完成前必须通过与改动范围匹配的验证计划。

### 8.4 结构化代码修改事务

代码修改的默认单位不应是模型任意截取的文本或“尽量大的代码块”，而应是**最小完整语法节点**。例如单个参数修改表达式节点，Flutter/Dart Widget 调整修改对应 Widget 子树，方法重写替换完整方法节点；Java 字段删除则把注解、修饰符、类型和字段声明作为同一节点。机械替换、生成文件和暂不支持语法树的语言可以回退文本 patch，但必须提高验证等级，不能静默降级。

现有能力作为兼容底座保留：

- `start/starts` 按完整括号块或 Python 缩进块定位，批量块统一使用原文坐标并拒绝重叠；
- `symbol_handle` v3 绑定项目、完整文件 SHA-256、稳定/位置节点 ID、原节点内容摘要、节点类型/精确范围和父节点范围，连续注解纳入声明节点；默认外部改写失败关闭，显式受控重定位也必须证明节点未变且唯一，旧 v1/v2 句柄保持严格模式兼容；
- `balance_guard` 在落盘前检查字符串/注释感知的 `{}()[]` 配平；
- undo、`preview_edit`、增量重索引和 [文件变更验证计划](./CHANGE_VERIFICATION.md) 负责预览、恢复与写后证据。

这些机制可以拦截常见的漏闭合符号和位置漂移，但括号配平不等于语法正确，构建后发现错误也不等于提前避免错误。下一阶段增加统一 `StructuredEditTransaction`（或 `CodeMutationGuard`），让 `write_file`、`edit_file`、`multi_edit`、`apply_patch`、LSP WorkspaceEdit 和 Agent Kernel 的代码写入走同一事务：

```text
结构索引定位 -> 生成节点级修改计划 -> 校验 file hash / node kind / parent range
             -> 在内存副本原子应用全部修改 -> 重解析完整变更文件
             -> 运行语言诊断与伴随节点清理 -> 格式化后再次解析
             -> 核对 diff 声明范围 -> 全部通过后落盘，否则不写入/整体回滚
```

事务必须满足以下不变量：

1. 修改不得引入新的语法诊断；原文件已有错误时，错误集合只能减少，或新增位置必须属于明确声明的修复范围；
2. 修改后的目标节点仍可解析，父节点边界不得意外漂移；有意替换父节点时必须在计划中显式声明；
3. 多节点、多文件重构必须全有或全无，任一步失败不得留下部分写入；
4. formatter 只负责规范化，不得作为修复残缺语法的手段；格式化后必须再次解析且结果幂等；
5. 最终 diff 只能覆盖事务声明的文件与节点；超范围变化、换行风格污染和无关格式化必须拒绝；
6. 语法门禁通过后仍需执行与风险匹配的静态检查、测试和构建，写入成功不能自证任务完成。

Java 需要独立的语义 adapter，优先接入 JDT Language Server，并以 `javac`、Maven 或 Gradle 构建作最终证据：

- 删除字段或方法时，绑定在声明上的 `@Resource`、`@Override` 等 annotation 随节点一起处理，禁止留下游离注解；
- annotation 删除后，仅在确认没有其他使用者时清理 `javax.annotation` / `jakarta.annotation` 等 import；
- 方法签名变化后重新解析类型层次：仍覆盖父类/接口时保留 `@Override`，不再覆盖时由语义诊断阻止提交或执行明确 quick fix；
- 字段注入删除后继续检查引用、构造器和框架约束，不能把“import 已清理”当作重构完成；
- 语言服务不可用时失败关闭到“内存预览 + 完整 Java 构建”，不得把轻量正则结果冒充语义事实。

Flutter/Dart、Rust、TypeScript/ArkTS 等语言采用相同协议，由语言 adapter 提供 parser、formatter、diagnostics 和 workspace edit；不要求 Docker。对百万级仓库，事务只加载目标节点、父节点和必要语义依赖，验证时解析受影响文件/模块，不把全仓正文送入模型。

分阶段实现与出口：

- **P0 写前门禁**：先在内存副本应用、配平、解析、diff 范围核对，失败不落盘；把现有写工具统一接入事务外壳；
- **P1 节点事务**：核心已落地——`symbol_handle` v3 封装稳定/位置 `node_id + expected_hash + node content hash + expected_kind + parent range`，支持单节点与同文件多节点原子修改；同文件非目标区域变化可显式受控重定位，目标变化、歧义或旧版句柄一律拒绝，不以模糊匹配静默越过冲突；
- **P1 Java 语义闭环**：JDT LS/Javac adapter、annotation/import/override 联动、Maven/Gradle 验证；
- **P2 跨文件事务**：承接 LSP WorkspaceEdit、原子暂存/提交、失败回滚和外部编辑冲突重规划；
- **评测出口**：固定加入漏 `) ] }`、Widget 子树错位、Java 游离 `@Override`/`@Resource`、误删仍在使用的 import、并发外部改写、部分多文件写入六类故障；要求新语法错误落盘率为 0、部分事务残留率为 0，并分别报告预防率、回滚率和误拒绝率。

### 8.5 有条件的多 Agent，而不是默认多 Agent

Trae Agent 的研究重点之一是 test-time scaling，通过生成、剪枝和选择多个候选提高 SWE-bench 成绩。HarmonyAgent 已有子 Agent/DAG，可借鉴但不应无条件并行：

- 只在任务复杂度、低置信度或高风险达到阈值时启用；
- 优先使用“探索者 + 实现者 + 审查者”三个有明确产物的角色；
- 每个 Agent 独立 worktree/沙箱，最终由测试和 grader 选择，不由另一个模型凭感觉投票；
- 用单 Agent 对照组衡量成功率增益是否值得额外成本。

## 9. 路线 E：发行与采用（P1/P2）

“没有二进制包”的评价已经过时，但它说明用户没有快速看到安装入口，或者没有建立对产物的信任。

建议：

- README 首屏放 Windows/macOS 下载按钮、版本、校验值和 3 分钟上手 GIF；
- Release 增加 SHA-256、SBOM、构建 provenance、签名状态和最小安装验证；
- macOS 完成 Developer ID 签名与 notarization，Windows 完成代码签名；
- 增加 Linux `.deb`/AppImage，或在文档中明确暂不支持，避免“跨平台”措辞超出产物；
- 每个 release 在全新 VM 上验证安装、首次启动、Provider 配置、打开示例工程、沙箱任务和自动更新；
- Python 只作为开发/评测辅助依赖，最终用户路径不要求 Python；
- 发布一份能力矩阵：哪些平台支持沙箱、Harmony SDK、设备、签名和 GPU embedding。

## 10. 12 周执行路线

### Phase 0：两周，先建立事实基线

交付：

- [x] 把当前 `sandbox_exec` 在 UI/文档中改称“临时副本试运行”，消除错误安全承诺；
- [x] 写 `SECURITY_BOUNDARY.md`：明确宿主、工作区、网络、凭据和 MCP 边界；
- [ ] 完成 headless eval adapter 的生产级 Agent Kernel 与一个真实模型 end-to-end 样例（builtin driver、`eval run --driver builtin`、11 个结构/文件/Git 工具、`SessionTrajectorySink`、`HeadlessToolRuntime`、原生异步 runner 主路径、可注入 `ModelClient`、完全离线的脚本 Provider tool-loop 测试、成本计量、超时/重试与失败/取消终态已实现；流式回合已落地——SSE 行缓冲 + `KernelStreamGovernor` 停滞治理 + `KernelStreamAccumulator` 组装严格 `KernelTurn`，停滞/提前关闭/坏帧/超限失败关闭，并保留 `reasoning_content` 供 reasoning-only 截断判断；参数级审批/L2 fail-closed、共享工具契约、trial 私有 `tool_runs` 与 `tool_metrics` 汇总也已接入，headless 工具执行已接入与 UI 相同的 `TOOL_POLICY` 退避 + `retryable_for` 谓词，并由 `max_tool_calls` 对全部尝试执行独立硬限制（重试次数入 trial 私有 `tool_runs.retry_count`；血缘式恢复编排为桌面会话特性，headless 无父运行血缘，按设计不接入）；统一 Kernel 的严格 `KernelTurn`、无 secret `KernelRequestPlan`、usage/cost ledger、acceptance stop gate、续写指令与 Provider transport 控制在 UI/headless 共用，run-config 单请求上限字段已落地，桌面 UI 流循环已切换到共用 `KernelStreamGovernor` + `KERNEL_STREAM_*` 常量，无 Docker 的真实 Provider 手动 workflow 已落地，单一外层 run-loop executor 仍待完成，见 [HEADLESS_AGENT_DRIVER.md](./HEADLESS_AGENT_DRIVER.md)）；
- [x] 固定 SWE-bench Verified 25 题 smoke 子集（v1 清单覆盖 12 个仓库和三档难度，固定官方 dataset revision；数量/唯一性/ID/revision 校验器与仓库清单测试已落地。官方 gold 25/25 容器自检只作为可选 CI 适配器验收，不是核心工具依赖，见 [SWE_BENCH_VERIFIED_25.md](./SWE_BENCH_VERIFIED_25.md)）；
- [x] 建立 10k/100k/1M 文件索引基准生成器，并记录 10k 当前基线；
- [x] 更新 README：二进制下载、支持平台和当前限制（badge 改为仅 Windows/macOS，并明确 Linux 暂不提供官方安装包，避免“跨平台”措辞超出实际产物）。

退出门槛：可以用一条命令重现当前的安全边界、检索性能和 25 题 Agent 基线。

### Phase 1：第 3—6 周，补核心底座

交付：

- [ ] 平台原生轻量 `SandboxBackend`；Shell/build/test 默认断网运行且不依赖 Docker（统一 `SandboxSpec`、后端契约、超时/取消、输出限制和审计事件已完成；现有 Docker/Podman `OciBackend` 保留为显式可选适配器，缺镜像/运行时会失败关闭，但不再计划设为桌面默认值；下一步实现 macOS/Linux/Windows 原生后端与 capability 探测）；
- [ ] approval 与 sandbox escalation 进入统一事件和审计链（审批决议已写入 `session_events`（`ToolApproval` 事件），沙箱升级在 `run_events`；`audit_timeline` 已把两者合并为一条按时间排序的统一审计时间线查询，审批/沙箱/工具调用同链可回放并进入 eval trajectory；headless 的 `SessionTrajectorySink` 已与完整迁移后的 trial 工具数据库共享同一连接，桌面运行时的写入侧进一步合并仍待做）；
- [ ] Host Capability Broker 原型，先覆盖 `hdc` 与 deploy（类型化窄能力 + 安全校验已落地为 `agent::capability_broker`——`HostCapability` 枚举覆盖 hdc 连接/断开/列表、install、deploy，`validate` 拒绝 shell 元字符/绝对路径/`..`/非 `.hap`；真实执行按 capability_id 接入 device_tools/build_tools 待真机）；
- [ ] 文件目录持久索引、watcher、Git diff 修复和分片；移除 4,000/400 静默截断（全库 SQLite 目录、状态/coverage、游标查询、原生 watcher、Git diff、事件直写和百万生成仓验收已完成；TS 系与 ArkTS Tree-sitter 已接入，必要时的物理分片待真实仓 SLO 触发）；
- [ ] `repo_query` 统一查询接口与 coverage/staleness 元数据（`search_symbols` 结构查询 MVP 已完成；`repo_query` 路由 MVP 已完成——`auto` 按查询形态分流 `path/symbol/concept` 到 lexical/结构索引并标注 `source_layer`，`impact` 模式已完成——精确图反向依赖返回“谁引用/调用了该符号”并按主流约定给出候选测试文件；依赖图重排的统一 planner 待完成）；
- [ ] 结构化代码修改事务 P0/P1：P0 已将 `write_file`、`edit_file`、`multi_edit` 与 LSP WorkspaceEdit 接入候选文本门禁；TS/JS/ArkTS 使用 Tree-sitter 错误增量检查，Java 在 JDT/Javac 接入前先以增量声明门禁阻止新增游离 `@Override`/`@Resource`，其他语言明确回退配平层；`multi_edit` 已先验证全部文件后原子提交并在写入失败时回滚，门禁/回滚/结构过期状态已接入工具卡。P1 节点句柄 v3 已携带 file hash、稳定/位置 node ID、节点内容摘要、kind、精确范围与 parent range，单节点和同文件多节点事务及显式受控重定位均已落地；Java/Kotlin 方法与字段及 Rust/Dart 专用轻量 adapter 已进入持久结构索引，Rust 属性/文档注释与 Dart 注解连续随声明修改。剩余工作是 Java JDT LS/Javac 的 import/override 类型语义联动；失败时不落盘或整体回滚，且不依赖 Docker；
- [ ] 每周真实模型回归，保存 patch/trajectory/cost/report。

退出门槛：恶意仓库脚本不能读取工作区外文件或联网；100k 文件仓库满足校准后的 P95 指标；结构化修改故障集中新增语法错误落盘率和部分事务残留率均为 0；真实模型评测可重复。

### Phase 2：第 7—12 周，提升成功率并公开证据

交付：

- [ ] Tree-sitter/ArkTS 容错 AST 层与依赖/影响图（AST、`contains`、语法级 `extends/implements`、保守直接 `calls`、同文件唯一目标、相对命名 import、根 `tsconfig` path alias、HarmonyOS `file:/link:` 本地包入口及有界命名/星号 re-export 闭包已完成；ArkTS LSP 唯一工程内定义和有界引用批次可增量沉淀成员调用边）；
- [x] LSP/SCIP 语义层和 fallback 策略（单点定义、单次最多 256 个引用的成员调用证据、目标扫描账本、覆盖率/截断/退避指标，以及方法优先的按需调度、空闲小批次调度、自适应占用预算和跨进程指数退避均已接入；SCIP importer 支持逐 document 有界解析、独立精确引用层、文件指纹失效与原子代次切换，并按 SCIP 语义把 `ForwardDefinition` 前向声明当作定义位置而非引用；热点符号关系超过单次 500 条上限时以 `relations_cursor` 按统一 keyset 逐页读取，并已记录关系查询 P50/P95 基准——5k 到 1M 条引用第一页 P50 稳定在约 3.6 ms，见 [INDEX_SCALE_BASELINE.md §7](./INDEX_SCALE_BASELINE.md)）；
- [ ] 延迟加载工具与程序化工具编排 A/B（`search_tools` 动态发现已落地——按 query 打分排序返回匹配工具，支持 `detail=name|summary`；常驻核心工具裁剪到 8—20 个、schema 选中后才进上下文、程序化编排与同模型 A/B 待做）；
- [ ] HarmonyBench v0（至少 50 题，其中一部分 holdout）；
- [ ] SWE-bench Verified 100 题与 SWE-Explore 报告；
- [ ] 首份公开 Agent Capability Report：成功率、成本、时延、安全和失败分类；
- [ ] Release 产物签名、SBOM、provenance 与新 VM smoke test。

退出门槛：相对 Phase 0，在固定模型与预算下，真实任务成功率有统计意义的提升；发布报告可由第三方复现。

### Phase 3：3—6 个月，形成领域壁垒

- [ ] HarmonyBench 100+，包含真机/模拟器可复现场景；
- [ ] SWE-bench Verified 全量或 SWE-bench Pro/Live 的里程碑报告；
- [ ] 本机轻量沙箱覆盖 Windows/macOS/Linux，能力不足时明确失败关闭；
- [ ] 百万文件仓库渐进索引与跨仓/跨模块语义查询；
- [ ] 基于真实失败分类做自动回流、A/B 和回归集扩充；
- [ ] 高风险任务采用有条件多 Agent 审查，证明收益/成本 Pareto 改善。

## 11. 建议立刻创建的首批 Issue / Epic

| 优先级 | Epic | 首个可合并切片 | 验收证据 |
| --- | --- | --- | --- |
| P0 | `SEC-01 Real Sandbox Boundary` | `SandboxBackend`、能力探测、HostDirect 风险标识 | 10 个逃逸负例 |
| P0 | `EVAL-01 Headless Agent Eval` | 单 task JSON -> patch/trajectory/report | CI artifact 可下载并复跑 |
| P0 | `INDEX-01 Large Repo Baseline` | 基准生成器 + 现实现报告 | 10k/100k/1M 数据 |
| P0 | `DOC-01 Truthful Capability Matrix` | 修正 sandbox/二进制/评测表述 | 文档漂移测试 |
| P1 | `SEC-02 Native Sandbox` | 平台原生 `network=none` + workspace scope + resource limits；OCI 可选 | 恶意脚本套件 0 escape，桌面安装不要求 Docker |
| P1 | `INDEX-02 Persistent File Catalog` | watcher + shard + Git checkout repair | 100k 增量 P95 |
| P1 | `CTX-01 Repo Query Planner` | lexical/symbol/LSP 路由和 coverage | SWE-Explore Recall@k |
| P1 | `TOOL-01 Deferred Tool Loading` | core tools + `search_tools` | 同模型 A/B |
| P1 | `HARMONY-EVAL-01` | 20 个真实脱敏任务 | hidden outcome graders |
| P2 | `REL-01 Trusted Releases` | checksum + SBOM + fresh-VM smoke | Release artifact |

## 12. 决策门槛：避免路线失控

每个阶段只看以下问题：

1. **安全**：模型生成代码能否越过声明边界？
2. **能力**：固定模型、固定预算下，resolved rate 是否提升？
3. **检索**：相关文件/行的 Recall@k 和时间是否提升？
4. **效率**：每个成功任务的成本、token、工具调用和 wall time 是否改善？
5. **领域价值**：HarmonyOS 任务是否显著优于通用 Agent + 通用工具？
6. **可复现**：第三方能否从 commit、镜像、配置、trajectory 和 grader 重现结论？
7. **修改完整性**：代码写入是否保持语法节点、语义依赖和事务边界完整，还是只能依赖后续构建发现残缺修改？

如果某项新架构不能改善至少一个指标，或改善幅度小于它带来的维护成本，就不应仅因为“主流项目也有”而合入。

## 13. 推荐的第一次评审议程

我们第一次一起评审时，建议只决定四件事：

1. 是否认同“安全执行、仓库理解、真实评测”是未来 12 周前三优先级；
2. Windows/macOS/Linux 原生后端分别能声明哪些能力，能力不足时如何失败关闭；
3. 通用评测先做 Verified 25/100，还是先做 HarmonyBench 20；
4. 是否把现有 `sandbox_exec` 立即改名并公开说明其真实边界。

建议默认选择：**平台原生轻量隔离先行，OCI 仅作外部 CI 适配器；Verified 25 与 HarmonyBench 20 并行建基线；立即修正文案。** 这条路径既保持桌面工具零 Docker 前置条件，也能产出可信、可复现、能对外回应评价的结果。

## 14. 资料来源

### 本项目证据

- [README：当前能力、安装包与技术栈](../README.md)
- [架构说明](ARCHITECTURE.md)
- [当前工具执行隔离边界](TOOL_ISOLATION.md)
- [`sandbox_exec` 当前实现](../src-tauri/src/agent/tools/quality_security.rs)
- [符号索引当前实现](../src-tauri/src/services/symbol_index.rs)
- [代码扫描与 `codebase_search`](../src-tauri/src/agent/scanner.rs)
- [固定评测集](FIXED_EVALUATION_SUITE.md)
- [评测 CI 门禁](EVALUATION_CI_GATES.md)
- [发布流水线](../.github/workflows/release.yml)
- [v2.1.1 GitHub Release](https://github.com/lookapu/HarmonyAgent/releases/tag/v2.1.1)

### 外部一手资料与公开基准

- [OpenAI：Harness engineering（2026-02-11）](https://openai.com/index/harness-engineering/)
- [OpenAI：Codex 默认沙箱与网络策略](https://openai.com/index/introducing-upgrades-to-codex/)
- [OpenAI：Agent SDK 的 harness/compute 分离与原生沙箱](https://openai.com/index/the-next-evolution-of-the-agents-sdk/)
- [Anthropic：Building effective agents](https://www.anthropic.com/engineering/building-effective-agents)
- [Anthropic：Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- [Anthropic：Advanced tool use / 延迟加载与程序化调用](https://www.anthropic.com/engineering/advanced-tool-use)
- [Anthropic：Demystifying evals for AI agents（2026-01-09）](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents)
- [SWE-bench 官方仓库与 Docker harness](https://github.com/SWE-bench/SWE-bench)
- [OpenAI：SWE-bench Verified 的构建与限制](https://openai.com/index/introducing-swe-bench-verified/)
- [SWE-bench 官方实验结果、预测、日志与轨迹格式](https://github.com/SWE-bench/experiments)
- [SWE-bench Pro](https://scale.com/blog/swe-bench-pro)
- [SWE-bench Live](https://swe-bench-live.github.io/)
- [SWE-Explore：单独评测仓库探索与代码定位](https://github.com/Qiushao-E/SWE-Explore-Bench)
- [GitHub Blackbird：代码搜索的 n-gram、分片、惰性迭代与增量摄取](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/)
- [Sourcegraph：大规模 monorepo 的后台索引、分片与分页](https://sourcegraph.com/docs/admin/monorepo)
- [Sourcegraph：Zoekt 索引、大文件边界与未索引搜索策略](https://sourcegraph.com/docs/admin/search)
- [Tree-sitter：增量解析](https://tree-sitter.github.io/tree-sitter/)
- [Sourcegraph：SCIP 精确代码导航](https://sourcegraph.com/docs/code-navigation/precise-code-navigation)
- [Trae Agent 官方仓库与技术报告入口](https://github.com/bytedance/trae-agent)
- [DeepSeek Harness 官方仓库](https://github.com/deepseek-ai/deepseek-harness)

---

本路线的核心不是证明外部评价“错了”，而是让下一位评价者能够用安装包、隔离测试、百万仓指标和公开 Agent 轨迹自行验证项目能力。
