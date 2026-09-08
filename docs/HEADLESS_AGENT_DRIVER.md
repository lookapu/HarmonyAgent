# 内置 Provider Headless Agent Driver 设计（可实现版）

> 状态：v2 设计 + Phase 0/1 已落地，Phase 4 的协议、预算、验收与 Provider transport 控制已开始共用
> 更新日期：2026-09-08
> 适用范围：`harmony-agent eval run --driver builtin`

当前代码已经提供 `HeadlessAgentDriver` 和 CLI 的 `--driver builtin` 分支：支持
OpenAI-compatible 流式 chat completions（SSE 字节级行缓冲 + `KernelStreamGovernor` 停滞治理 +
`KernelStreamAccumulator` 帧归一化，`stream_options.include_usage` 请求 usage 尾帧）、
11 个受限工具（结构/文本检索、文件读写/精确编辑、Git 只读）、事件记录、轮次/墙钟限制和
基础策略拒绝；`SessionTrajectorySink` 会把 session event 与 trajectory 同步写入，事件流会记录
`mode=minimal`。runner 主路径已经原生异步化，外部进程 adapter 在 blocking worker 中运行；
模型调用通过可注入的 `HeadlessModelClient` 边界，离线脚本 Provider 测试可以完整执行
`write_file` tool loop，不需要网络或 Docker。流式读取对停滞、无结束标记提前关闭、空流、
坏帧和超限正文全部失败关闭；停滞错误归类为 Network，自动进入与 UI 共用的指数退避重试。
它仍然不等价于 Tauri UI 的完整 Agent loop；下文的 Phase 2/3/4 是后续演进要求。

## 1. 结论先行

内置 Provider headless driver 值得推进，但它不是简单的 Provider 搬迁，也不是
`ProcessAgentDriver` 的小包装。它需要把 Agent 的异步模型调用、工具执行、权限治理、
事件审计和取消语义放到一个可复用的内核边界中。

本设计采用以下原则：

1. `ProcessAgentDriver` 保留，继续作为外部 adapter 和回归对照组。
2. builtin driver 先支持一个 OpenAI-compatible 协议，再扩展其他协议。
3. `worktree` 只代表代码工作区隔离，不代表安全沙箱。
4. 模型网络和任务网络分离：允许访问明确的 Provider endpoint，禁止任务工具任意联网。
5. headless 的最小 loop 可以先落地为 smoke test，但只有接入统一 Agent Kernel 后才能用于可比的正式 benchmark。
6. 所有停止、工具调用、审批拒绝、Provider 错误和资源消耗都必须可审计、可回放。

## 2. 目标与非目标

### 2.1 目标

- 为 eval CLI 增加原生 `HeadlessAgentDriver`。
- 使用真实模型、真实工具和真实 grader 完成端到端 trial。
- 复用 HarmonyAgent 的工具契约、权限策略、验收、事件和恢复语义。
- 保留确定性 stub provider，支持离线单测和故障注入。
- 让 headless 与 Tauri UI 最终共享同一个 Agent Kernel，避免双 loop 长期漂移。

### 2.2 非目标

- 第一阶段不同时实现 OpenAI、Anthropic、Gemini 三套协议。
- 第一阶段不把 `stream_chat_inner` 整体搬出 `chat.rs`。
- 没有 OCI/容器沙箱时，不把 builtin driver 宣称为不可信代码执行环境。
- 不把 `acceptance.passed` 当作最终评测结果；最终结果必须由 clean worktree 中的 grader 决定。
- 不在 run-config 或日志中保存 API key、完整 Authorization header 或敏感环境变量。

## 3. 当前实现事实

现有接口和实现对设计有几个硬约束：

| 事实 | 影响 |
| --- | --- |
| legacy `AgentDriver::run` 是同步函数 | 已不再作为 runner 主路径；只由外部进程 adapter 内部兼容，并放入 blocking worker |
| `AgentDriverOutcome.trajectory` 是 `Vec<TrajectoryEvent>` | 现有 `session_events_to_trajectory` 是写入 `TrajectoryWriter`，不是返回 Vec，不能声称 runner 零改动 |
| `run_tool` 需要 `DbState`、`McpManager` 和 `ToolCtx` | `ToolCtx::empty()` 只解决 AppHandle 缺失，不会解决数据库、项目和权限上下文 |
| 部分工具依赖 `db::global()` | headless 临时数据库不能简单替换全局数据库；并行 trial 需要消除隐式全局状态 |
| `prepare_worktree` 是 Git 工作区隔离 | 不提供进程、文件系统、网络和环境变量级安全边界 |
| 权限不仅由工具名决定 | `run_command`、release build 等操作还需要参数级 fresh approval 判断 |
| runner 按 `cost_cny` 进行限制 | 未知成本填 0 会造成成本约束失效，必须显式表达成本状态 |

因此，本文不再承诺“`run_trial` 零改动”，而是定义必要的最小接口演进。

## 4. 目标架构

```text
eval CLI / Tauri UI
          │
          ▼
      AgentKernel  <────────────── EventSink
       │   │   │
       │   │   └── Acceptance / StopPolicy / Recovery
       │   ├────── PolicyEngine
       └────────── ModelClient
          │
          ▼
      ToolRuntime
       ├── workspace scope
       ├── network policy
       ├── subprocess env allowlist
       ├── timeout / cancellation
       ├── DbState / MCP context
       └── tool contracts
```

### 4.1 组件职责

- `ModelClient`：协议请求、流式响应、usage、重试和 Provider 错误。
- `AgentKernel`：消息历史、工具调用循环、阶段推进、停止原因和恢复语义。
- `PolicyEngine`：工具静态契约、参数级审批、workspace scope、网络策略。
- `ToolRuntime`：在受控上下文中执行工具，处理超时、取消、线程和结果脱敏。
- `EventSink`：统一追加 User/Assistant/Tool/Policy/System/Usage 事件。
- `Acceptance`：内部“是否可以停止”信号，不替代最终 grader。
- `EvalRunner`：工作树、patch、grader、报告和产物编排。

## 5. AgentDriver 接口演进

当前同步接口不能承载 Provider 流和工具异步执行。Phase 0 应将 driver 改为异步边界：

```rust
#[async_trait]
pub trait AgentDriver: Send + Sync {
    async fn run(
        &self,
        task: &EvalTask,
        workspace: &Path,
    ) -> Result<AgentDriverOutcome, AgentDriverError>;
}
```

如果不引入 `async_trait`，使用 `Pin<Box<dyn Future<Output = ...> + Send + '_>>` 等价实现。
`run_trial` 也随之异步化；CLI 只在最外层创建一次 Tokio runtime，禁止在已有 runtime 内部
使用嵌套 `block_on`。

当前代码以 `AsyncAgentDriver` 作为 `run_trial` 的主接口，并由 builtin driver 与
`ProcessAgentDriver` 实现；旧的同步 `AgentDriver` 只保留为外部进程 adapter 的内部兼容实现，
进程等待通过 `spawn_blocking` 与异步 runtime 隔离。

`agent::agent_kernel` 已成为 UI/headless 共用内核的第一个代码切片：
`parse_openai_turn` 把 Provider 响应严格转换为 `KernelTurn`/`KernelToolCall`，
`KernelUsageLedger` 统一累计 input/output/cached token 与成本。设有成本上限但 Provider
缺少 usage 时会失败关闭；成本超限通过 `agent_budget_stop` 正常收敛，不会提前返回而丢失 trajectory。

第二个切片已把“模型申请停止”统一为 `KernelStopDecision`：桌面 UI 与 builtin headless
共同使用 `Accepted / Remediate / Exhausted` 裁决。headless 会编译 `GoalContract`、累计
真实工具证据，并在缺少写入后验证等证据时向模型注入有界补救提示；最终报告写入
`agent_acceptance_final`，失败则标记 `acceptance_failed`，grader 仍是 trial 成败的最终裁判。

第三个切片 `KernelStreamAccumulator` 已接管桌面 UI 的 Provider 流式帧归一化：统一检测
OpenAI/Anthropic/Gemini 的正常结束与 token 截断，以常量空间累计 usage，并按 index 合并
OpenAI 原生工具调用分片。Anthropic 分散在 `message_start` 与 `message_delta` 的输入/输出/
缓存 token 会合并为同一份 usage；一个 OpenAI 帧内的多个并行 tool call 不再只保留首个。

第四个切片 `KernelRequestPlan` 已统一桌面流式、桌面非流式和 headless 的请求规划：协议端点、
请求体、鉴权类型、采样参数、原生工具 schema 与 DeepSeek reasoning 历史净化只有一套实现。
计划对象刻意不携带 API key，transport 只在发送前注入凭据；非法 temperature/top-p/max_tokens
在内核失败关闭。`run_provider_transport` 已统一 UI/headless 的指数退避、Retry-After、尝试次数、
取消轮询与绝对截止时间；调用方只注入协议 HTTP attempt、取消来源和 watchdog touch。代理选择与
流响应读取/停滞治理仍由各 adapter 负责，下一阶段继续收敛。

第五个切片 `KernelStreamGovernor` 已把流停滞治理提取为共用状态机：静默超时、reasoning-only
宽限封顶与响应字节预算只有一套策略，时间由调用方注入（可离线单测与故障注入），语义对齐
桌面 UI 的流循环（有效产出刷新停滞线、纯思考流封顶在首次思考 + 宽限期、首字节初始化）。
headless 流式回合是它的首个真实消费者：SSE 字节级行缓冲（`KernelSseBuffer`，多字节字符
跨 chunk 不损坏）+ governor + `KernelStreamAccumulator` 组装严格 `KernelTurn`，停滞/提前
关闭/坏帧/超限全部失败关闭。桌面 UI 的流循环尚未切换到该组件，是剩余收敛项。

### 5.1 事件输出接口

推荐新增事件 sink，而不是让 driver 组装完整 trajectory Vec：

```rust
pub trait AgentEventSink: Send {
    fn append(&mut self, event: &TrajectoryEvent) -> Result<(), String>;
}
```

headless 可同时实现：

- `SessionEventSink`：写入 `session_events`；
- `TrajectorySink`：写入 `trajectory.jsonl`；
- `CompositeSink`：一次写入多个审计后端。

如果暂时不改 `AgentDriverOutcome`，headless 必须从 session events 回放出 Vec；不能把
“写入 `TrajectoryWriter`”描述成“返回 trajectory”。长期应让 runner 直接拥有 sink。

## 6. Provider 抽象与凭据

### 6.1 先做一个协议

Phase 1 只支持 OpenAI-compatible streaming。抽象保持协议无关：

```rust
pub trait ModelClient: Send + Sync {
    fn stream<'a>(
        &'a self,
        request: ModelRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, ModelError>> + Send + 'a>>;
}
```

Anthropic/Gemini 在 ModelClient 稳定后分别增加适配器，不在第一阶段同时搬迁三套协议。

### 6.2 配置分离

运行时秘密从环境变量或 secret provider 读取；可复现指纹进入 manifest，但不保存秘密：

```text
HARMONY_EVAL_PROVIDER_ID
HARMONY_EVAL_BASE_URL
HARMONY_EVAL_API_KEY
HARMONY_EVAL_PROTOCOL=openai
HARMONY_EVAL_MODEL_ID
HARMONY_EVAL_INPUT_PRICE_CNY_PER_1K
HARMONY_EVAL_OUTPUT_PRICE_CNY_PER_1K
```

必须校验：

- provider、protocol、model_id 与 `EvalRunConfig.model` 一致；
- base URL 使用 HTTPS，或显式允许本地测试 endpoint；
- endpoint 记录规范化后的 host/path 指纹；
- 请求 ID、协议版本、模型版本和 pricing version 写入 report；
- API key 不进入 task JSON、run-config、manifest、trajectory、stderr 或子进程环境。
- 设置了 `max_cost_cny` 的任务必须同时提供两个价格快照；否则 builtin driver fail-closed。

CLI 启动 builtin 时会实际校验 provider、model_id、protocol 与 run-config；不一致直接拒绝执行，避免 manifest 与真实请求漂移。

### 6.3 密钥与子进程隔离

模型客户端读取 key 后，任务工具不得继承完整宿主环境。工具进程只能获得显式 allowlist：

- `PATH`、`HOME`（如确实需要，使用任务专用目录）、语言运行时必要变量；
- 工作区路径和任务专用临时目录；
- 不包含 Provider key、云端 token、SSH key、代理认证信息。

不要通过全局 `set_var`/`remove_var` 在多线程进程中清理环境；使用 `Command::env_clear()`
后逐项注入，或在更高层使用受控 secret handle。

## 7. Headless ToolRuntime

`ToolCtx::empty()` 可以作为 AppHandle 缺失时的基础值，但不能作为完整 headless runtime。
当前代码已新增 `HeadlessToolRuntime`，集中持有数据库、workspace、MCP、取消标记和策略：

```rust
pub struct HeadlessToolRuntime {
    pub db: DbState,
    pub project_id: String,
    pub project_root: PathBuf,
    pub mcp: McpManager,
    pub policy: ToolExecutionPolicy,
    pub cancellation: CancellationToken,
}
```

要求：

- 使用完整 migration 初始化 trial 数据库；
- 创建项目、run、conversation 等必要记录；
- 不依赖 `db::global()` 读取 trial 状态；
- MCP 默认关闭，只有显式 allowlist 才能启用；
- 所有相对路径解析到 `project_root` 或任务临时目录；
- 禁止路径逃逸、符号链接逃逸和工作区外写入；
- 工具执行使用硬超时、协作取消和 stuck 归因；
- 工具返回统一脱敏后再写事件和模型上下文。

并行 trial 前，必须先完成全局 DB 依赖清理；否则 builtin trial 默认串行执行。

## 8. 统一 AgentKernel 循环

### 8.1 第一阶段最小循环

```text
prepare
  ├─ validate task / run config / provider
  ├─ create HeadlessToolRuntime
  ├─ create EventSink
  ├─ append user message
  └─ initialize StopPolicy and budget

round
  ├─ build ModelRequest(system + history + tool schemas)
  ├─ stream ModelResponse
  ├─ append assistant/reasoning/usage event
  ├─ parse native tool calls
  ├─ for each call:
  │    ├─ PolicyEngine.check(tool, args, workspace, network)
  │    ├─ reject approval/unknown/out-of-scope calls
  │    ├─ ToolRuntime.execute for allowed calls
  │    ├─ append policy/tool call/tool result events
  │    └─ append result to model history
  ├─ Acceptance.can_stop is only a candidate stop signal
  └─ enforce steps/tokens/cost/wall-time/cancellation

finish
  ├─ append stop reason and usage summary
  ├─ flush event sink
  └─ return outcome; final resolved status comes from grader
```

### 8.2 停止语义

必须区分：

- `agent_stop_reason=acceptance_passed`：Agent 认为可以停止；
- `trial_outcome=resolved`：clean worktree grader 通过；
- `trial_outcome=unresolved`：grader 未通过或 Agent 未完成；
- `harness_error`：Provider、工具 runtime、事件或配置错误；
- `cancelled`：用户/预算/墙钟取消。

纯文本回复可以触发 Agent 停止候选，但不能直接标记 trial resolved。

## 9. 权限与网络策略

### 9.1 工具审批

不能只按工具名把 L0/L1 一律放行。实际策略为：

- L0 且只读：允许，但仍受 workspace、网络和超时限制；
- L1：仅允许显式列入 headless allowlist 且作用域在工作树内；
- L2：拒绝；
- `requires_fresh_explicit_approval(tool, args)` 为 true：拒绝；
- 未知工具和默认 MCP 工具：拒绝；
- `ask_user`、交互等待和需要 Tauri UI 的工具：拒绝并记录原因。

每次拒绝必须追加结构化 `ToolApproval { approved: false, reason }` 事件，递增
`policy_violations`，不能静默回退到宿主执行。

builtin 当前已通过 `HeadlessToolPolicy` 对 allowlist 外工具追加 `ToolApproval(approved=false)` 和
`tool_rejected` trajectory 事件；工具自身失败追加 `tool_error`，Provider 截断追加
`provider_truncated`，轮次耗尽追加 `max_steps_exceeded`。

策略检查不再只判断工具名：它会复用 `permissions::requires_fresh_explicit_approval()`、
`permissions::tool_level()` 与 `tools::contracts::contract()`。非法 JSON、非 object 参数、
L2、逐次审批工具和 allowlist 外工具全部 fail-closed；允许调用的副作用类型、恢复策略与
契约超时会进入审批事件。工具硬超时取契约 timeout 与任务剩余 wall time 的较小值。

### 9.2 网络分层

`network=none` 只适用于任务执行面：

```text
model_network = allowlist(HARMONY_EVAL_BASE_URL)
tool_network  = none
```

如果任务需要联网工具，必须显式声明 allowlist、凭据范围和审计字段；不能用一个字符串
同时表达模型网络和工具网络。

### 9.3 隔离边界声明

Git worktree 只提供代码目录隔离，不是安全沙箱。平台原生轻量 sandbox backend 未完成前，builtin driver
只适合受信仓库和手动 smoke test，不适合运行恶意或不可信代码；核心工具不以 OCI/容器作为前置依赖。

## 10. 事件、轨迹与可复现性

事件是审计真源，至少覆盖：

- user message；
- assistant message/reasoning 摘要；
- model request/response metadata；
- tool call/result；
- policy approval/rejection；
- usage、cost status、retry、timeout、cancellation；
- stop reason、failure taxonomy。

事件 payload 必须脱敏，避免写入 API key、Authorization、完整环境变量和大段重复文件内容。
大内容使用 digest、路径、范围和大小引用；需要时通过 artifact 文件保存。

当前 builtin 会把回填模型历史的单个工具结果限制为 32,000 字符，按 Unicode 字符安全截断；
截断会写入 `tool_output_truncated` failure taxonomy，并在 ToolResult/trajectory 中记录
`truncated=true`。事件 payload 仍只保存前 4,000 字符，避免审计文件膨胀。

`HeadlessToolRuntime` 会在每个 trial 的内存 SQLite 中执行完整应用 migrations；
`SessionTrajectorySink::from_db` 与工具 runtime 共享该连接，因此审批、工具相关状态和
session events 不再写入彼此隔离的临时数据库。trajectory 仍作为同一次事件追加的派生输出返回 runner。
每个 trial 同时拥有独立 project/conversation/trace scope，实际执行的工具调用会写入
`tool_runs` 终态，再由现有 `tool_metrics::summary()` 生成 `tool_metrics` trajectory 事件，
统一统计成功率、参数错误率、超时率、取消数和平均耗时。
ToolCall/ToolResult 审计不保存无界原文：JSON 会先按敏感字段递归脱敏，自由文本再走统一
文本遮罩，事件仅保留 4,000 字符预览、原文 SHA-256 和独立截断标记；模型上下文的
32,000 字符上限与审计预览上限分别记录，避免混淆。

`session_events_to_trajectory` 的当前实现写入 `TrajectoryWriter`。因此采用以下过渡策略：

1. Phase 0：由 `SessionTrajectorySink` 同时维护 session events 与 outcome trajectory；
2. Phase 1：runner 继续写 `trajectory.jsonl`，但由统一 sink 负责；
3. Phase 3：删除 driver 对 trajectory 文件格式的直接认知。

每个 trial 必须产出：

- `manifest.json`；
- `report.json`；
- `trajectory.jsonl`；
- `model.patch`；
- grader stdout/stderr；
- 可选 artifact bundle。

## 11. 计量、预算与取消

### 11.1 成本

新增显式字段：

```text
cost_status = exact | estimated | unavailable
pricing_version
```

当任务设置 `max_cost_cny` 时：

- `exact`：按真实价格硬限制；
- `estimated`：按记录的价格快照限制，并标记估算；
- `unavailable`：fail-closed，不得用 0 伪装未消耗。

### 11.2 预算

同时限制：

- rounds/steps；
- input/output tokens；
- tool calls；
- wall-clock；
- cost；
- 单次 Provider 请求和单次工具执行时间。

当前 builtin 把任务剩余 wall time 作为 Provider 请求、响应读取和退避共享的绝对截止时间，
并对错误响应做文本脱敏。HTTP 429、5xx、连接失败和传输超时与 UI 共用
`STREAM_REQUEST_POLICY` 指数退避策略，尊重 Retry-After，实际尝试次数写入
`AgentDriverOutcome.retries`；工具 allowlist 内外分别记录
`ToolApproval(approved=true/false)`。单请求上限已作为独立 run-config 字段落地：
`EvalRunConfig.request_timeout_seconds`（可选，1..=3600，不得超过任务 wall time），
CLI 传给 builtin driver，实际生效值取它与剩余 wall time 的较小值并写入
`trial_started`/`driver_started` 事件；缺省时使用内置 60 秒默认值。

Provider 流、工具执行和子进程都必须接受 `CancellationToken`。不能只依赖外层
`run_trial`，因为同步阻塞会绕过 wall-time 保护。

## 12. 分阶段落地

每阶段一个独立提交，阶段末必须可编译、可测试、可回滚。

### Phase 0：接口和安全边界

- 异步化 `AgentDriver` / `run_trial`；
- 新增 `AgentEventSink` / `SessionTrajectorySink`；
- 新增 `HeadlessToolRuntime`（数据库、workspace、MCP、取消和策略边界）；
- 明确 model/tool 双网络策略；
- 子进程环境 allowlist；
- Provider 配置校验和敏感字段脱敏；
- 增加 stub provider、stub tool runtime 测试。

验收：不搬迁 `chat.rs` 大块代码，现有 eval stub 测试通过。

### Phase 1：最小 builtin loop

- 只支持 OpenAI-compatible stub/真实客户端；
- 支持有限工具 allowlist；
- 接入事件 sink、预算、取消和拒绝策略；
- 完成一个离线 `a.txt -> fixed` 端到端测试；
- 产出四件套和完整失败分类。

验收：stub 任务可重复通过；审批工具必定拒绝；API key 不出现在任何产物中。

### Phase 2：CLI 与真实模型 smoke

- `harmony-agent eval run --driver builtin`；
- 真实 Provider 手动 workflow；
- 仍标记 `driver_mode=minimal`；
- 不进入稳定 benchmark 门禁。

验收：5 分钟小仓任务能得到可审计 trial；超时、断网、Provider 错误可恢复并归因。

### Phase 3：统一治理与成本

- 接入参数级审批、tool metrics、recovery 和 coordinator；
- 接入 pricing snapshot；
- 清理 headless 路径上的 `db::global()` 隐式依赖；
- 为并行 trial 建立独立数据库和资源作用域。

验收：headless 与 UI 对同一工具的权限、超时、重试和事件语义一致。

### Phase 4：统一 Agent Kernel

- 从 `chat.rs` 抽取 ModelClient/stream parser（`KernelTurn`、usage ledger、多协议 `KernelStreamAccumulator`、无 secret 的 `KernelRequestPlan` 与共用 `run_provider_transport` 已落地；流响应读取/停滞治理仍待抽取）；
- 抽取消息历史、tool loop、reflexion、governance、recovery（acceptance stop gate 已共用）；
- Tauri UI 和 eval 共用 AgentKernel；
- 再扩展 Anthropic/Gemini 适配器。

验收：同一 stub provider 脚本在 UI adapter 和 headless adapter 上产生等价决策轨迹。

## 13. 测试策略

### 单元测试

- Provider SSE 分块、tool call 拼接、usage 和错误解析；
- provider 配置一致性和敏感字段脱敏；
- PolicyEngine 的 L0/L1/L2、参数级 fresh approval、路径越界和网络拒绝；
- cost status 与预算边界；
- cancellation、timeout、stuck、retry；
- EventSink 顺序、重放和 digest。

### 集成测试

- stub provider + 真实 `ToolRuntime` 修改文件并通过 grader；
- 拒绝 `run_command`、`git_push`、MCP 和 `ask_user`；
- 外部工具子进程无法读取 Provider key；
- provider 断流、工具超时、进程崩溃后生成完整失败 bundle；
- 两个 trial 不能互相读取 DB、事件或工作树。

### 真实模型 smoke

只作为手动 workflow：

- 小仓库；
- 固定模型和 endpoint；
- 显式预算；
- 不提交 key；
- 不作为稳定 CI 门禁，避免计费和模型漂移影响主线。

## 14. 主要风险与决策记录

| 风险 | 决策 |
| --- | --- |
| 双 loop 漂移 | Phase 1 允许 minimal 标识；Phase 4 必须统一 AgentKernel |
| 临时 DB 与全局 DB 冲突 | builtin 默认串行；完成显式 runtime 后再开放并行 |
| API key 泄漏到工具 | 子进程环境 allowlist，禁止继承完整环境 |
| worktree 被误认为沙箱 | 文档、CLI 和 report 明确标记安全边界 |
| 成本未知 | `cost_status` 显式表达，不能用 0 伪装 |
| acceptance 误判成功 | acceptance 只控制 Agent 停止候选，grader 决定 trial outcome |
| 多协议范围过大 | 先 OpenAI-compatible，再逐个加协议 |

## 15. 与既有文档衔接

- `AGENT_EVAL_HARNESS.md`：本设计补充 builtin driver 的运行时和安全边界；正式结果仍遵守其 manifest、trajectory、patch、grader 产物规范。
- `AGENT_EVOLUTION_ROADMAP_2026.md`：本设计对应 headless eval adapter，但 Phase 0 先完成接口和安全基础，不直接搬迁整个 UI loop。
- `AgentDriver` 的最终目标不是另造一套 Agent，而是让 UI adapter 和 eval adapter 共享 `AgentKernel`。

## 16. 当前执行建议

下一步推进 Phase 2/3，但保持核心工具不依赖 Docker：

1. 真实 Provider 手动 smoke workflow 与脱敏产物检查已落地
   （`headless-eval-smoke` workflow + `AGENT_EVAL_HARNESS.md` 产物规范）；
2. 流响应读取/停滞治理已在 headless 落地（SSE 行缓冲 + `KernelStreamGovernor` + 失败关闭）；
   下一步把桌面 UI 流循环切换到同一组件，并继续抽取消息历史/tool loop；
3. 将参数级审批、tool metrics 和 recovery 接入 headless runtime（参数级审批与
   tool metrics 已接入；recovery 仍待接入）；
4. 清理 headless 工具路径上的全局数据库依赖；
5. 最终抽取 UI/headless 共用的 Agent Kernel。
