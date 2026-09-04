# HarmonyAgent Agent 能力演进路线（2026）

> 状态：讨论稿，不代表已经承诺实施  
> 调研日期：2026-09-03  
> 基线版本：`main` / `v2.1.1` / `8d7443e`  
> 目标：把外部评价转化为可验证的产品与工程路线，而不是继续堆叠功能清单。

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

## 5. 路线 A：真正的默认沙箱（P0）

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

1. **先实现 OCI 后端**：Docker/Podman 均可，最容易建立可验证的文件系统、网络和资源边界，也能复用 SWE-bench 官方容器。
2. **再实现本机轻量后端**：按平台封装原生限制；如果某平台达不到声明能力，UI 必须显示“不受保护”，不能仍叫 sandbox。
3. **保留 `HostDirectBackend`**：只作为显式兼容模式，启动任务时持续显示风险，不作为默认值。
4. **拆分宿主特权工具**：`hdc`、模拟器、签名、发布走 Host Capability Broker，每次只获得完成该动作所需的句柄和范围。

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

Agent 的默认入口采用 [Structure-first 代码导航](./STRUCTURE_FIRST_NAVIGATION.md)：先把符号按 `entity`（类/组件/类型/状态等）和 `logic`（函数/方法）组织并分页检索，再按返回的结构行区间读取正文。二分类只用于规划，索引仍保留语言原生 kind；索引无结果或 coverage 不完整时必须走 lexical/LSP fallback。现有 `search_symbols` 已加入签名、父级、起止行、角色、稳定游标、coverage 和 staleness；TS/TSX/JS/JSX 节点进一步标注 `tree_sitter` 来源并使用 AST 精确范围，其余语言或语法错误文件明确标注 `lightweight` fallback。

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
- 路径/符号/自然语言查询 P50/P95；
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

1. **SWE-bench Verified smoke（25 题）**：打通官方 Docker grader；目标是可复现，不追榜。
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

### 8.4 有条件的多 Agent，而不是默认多 Agent

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
- [ ] 完成 headless eval adapter 设计与一个真实模型 end-to-end 样例（接口设计已完成，真实 runner/样例待实现）；
- [ ] 固定 SWE-bench Verified 25 题 smoke 子集；
- [x] 建立 10k/100k/1M 文件索引基准生成器，并记录 10k 当前基线；
- [x] 更新 README：二进制下载、支持平台和当前限制。

退出门槛：可以用一条命令重现当前的安全边界、检索性能和 25 题 Agent 基线。

### Phase 1：第 3—6 周，补核心底座

交付：

- [ ] `SandboxBackend` + OCI 实现；Shell/build/test 默认断网运行（已完成 `SandboxSpec`、后端契约、Docker/Podman 运行时探测、fail-closed OCI argv、超时/取消清理、输出限制和审计事件；实际命令接线与 artifact 导出待完成）；
- [ ] approval 与 sandbox escalation 进入统一事件和审计链；
- [ ] Host Capability Broker 原型，先覆盖 `hdc` 与 deploy；
- [ ] 文件目录持久索引、watcher、Git diff 修复和分片；移除 4,000/400 静默截断（全库 SQLite 目录、状态/coverage、游标查询、原生 watcher、Git diff、事件直写和百万生成仓验收已完成；TS 系与 ArkTS Tree-sitter 已接入，必要时的物理分片待真实仓 SLO 触发）；
- [ ] `repo_query` 统一查询接口与 coverage/staleness 元数据（`search_symbols` 结构查询 MVP 已完成，统一 planner 待完成）；
- [ ] 每周真实模型回归，保存 patch/trajectory/cost/report。

退出门槛：恶意仓库脚本不能读取工作区外文件或联网；100k 文件仓库满足校准后的 P95 指标；真实模型评测可重复。

### Phase 2：第 7—12 周，提升成功率并公开证据

交付：

- [ ] Tree-sitter/ArkTS 容错 AST 层与依赖/影响图（AST、`contains`、语法级 `extends/implements`、保守直接 `calls`、同文件唯一目标、相对命名 import、根 `tsconfig` path alias、HarmonyOS `file:/link:` 本地包入口及有界命名/星号 re-export 闭包已完成；成员调用需 LSP/SCIP 语义证据）；
- [ ] LSP/SCIP 语义层和 fallback 策略；
- [ ] 延迟加载工具与程序化工具编排 A/B；
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
| P1 | `SEC-02 OCI Sandbox` | `network=none` + workspace mount + resource limits | 恶意脚本套件 0 escape |
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

如果某项新架构不能改善至少一个指标，或改善幅度小于它带来的维护成本，就不应仅因为“主流项目也有”而合入。

## 13. 推荐的第一次评审议程

我们第一次一起评审时，建议只决定四件事：

1. 是否认同“安全执行、仓库理解、真实评测”是未来 12 周前三优先级；
2. 默认沙箱先走 OCI，还是优先做 Windows/macOS 本机后端；
3. 通用评测先做 Verified 25/100，还是先做 HarmonyBench 20；
4. 是否把现有 `sandbox_exec` 立即改名并公开说明其真实边界。

建议默认选择：**OCI 沙箱先行；Verified 25 与 HarmonyBench 20 并行建基线；立即修正文案。** 这条路径最快产生可信、可复现、能对外回应评价的结果。

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
