# 内置 Provider Headless Agent Driver 设计（可实现版）

> 状态：Phase 0—3 与 Phase 4 A—AF 已落地；UI/headless 已共享请求、流治理、预算、验收、历史组装策略、循环治理与 executor 状态所有者，单一 IO run-loop 仍是后续收敛项
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
它仍然不等价于 Tauri UI 的完整 Agent loop：两个 adapter 已共享关键策略组件，但尚未由同一个 run-loop executor 驱动。

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
关闭/坏帧/超限全部失败关闭。桌面 UI 的流循环也已切换到同一 governor；adapter 仍分别负责
协议 IO 与 UI/CLI 生命周期。

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

Task schema v1 还支持可选 `limits.max_tool_calls`（缺省 400，合法范围 1—10,000）。该预算
统计模型提出的全部工具调用，包括被权限策略拒绝、被循环治理拦截和实际执行的调用；下一次
调用越界时写入 `agent_tool_budget_stop`，以 `max_tool_calls_exceeded` 收尾，不再等待 round
或重复调用硬上限触发。

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

**Phase 4 实施状态（2026-09-09 更新）**：

| 阶段 | 内容 | 状态 |
|---|---|---|
| A | 纯策略组件落盘（kernel_loop/kernel_history + 黄金单测） | ✅ COMPLETED |
| B | 工具重试共用（run_tool_with_retry 平移进 agent_kernel） | ✅ COMPLETED |
| C | UI 接入 KernelLoopGovernor + KernelToolBudgetGate | ✅ COMPLETED |
| D | UI 接入 KernelRoundRouter | ✅ COMPLETED |
| E | 消息组装入核（E1 中段 → E2 图片 → E3 压缩决策） | ✅ COMPLETED |
| F | headless 闭合防护缺口（governor/router 集成 + ScriptedClient 请求记录 + 5 个集成测试） | ✅ COMPLETED |
| G | 差分测试 + 文档收口（2 个差分测试验证 router 黄金轨迹与 driver 事件序列一致） | ✅ COMPLETED |
| H | 独立工具调用预算 + reasoning 流回放 + UI/headless 共用续写指令 | ✅ COMPLETED |
| I | 单轮决策收敛（`KernelRoundDecision = notices + 唯一 control`，移除 adapter 动作序列推断） | ✅ COMPLETED |
| J | 唯一终止状态（`KernelRunState` 锁定首个主原因，终止原因进入最终事件） | ✅ COMPLETED |
| K | executor 状态所有权收敛（UI/headless 共用 `KernelExecutorState` 持有 router/governor/termination） | ✅ COMPLETED |
| L | Provider 请求前安全点（统一 deadline/cancel 优先级与剩余墙钟时间） | ✅ COMPLETED |
| M | executor 回合/工具尝试记账（固定工具硬上限裁决入核，UI 保留动态预算） | ✅ COMPLETED |
| N | 统一 executor 最终快照（具名 round counters + 桌面/headless 同结构审计） | ✅ COMPLETED |
| O | 最终快照失败关闭（终止原因必填，未终止/未跑满禁止伪造 final snapshot） | ✅ COMPLETED |
| P | 停止验收补救状态入核（UI/headless 共用 `decide_stop` 与 remediation 计数） | ✅ COMPLETED |
| Q | acceptance 单一职责（gate 只管契约/证据报告，补救状态仅由 executor 持有） | ✅ COMPLETED |
| R | 终态策略自动归因（空轮耗尽/最终工具循环熔断在裁决处写入 run termination） | ✅ COMPLETED |
| S | executor 终态吸收（安全点保留首因，终止后拒绝新增 Provider 回合） | ✅ COMPLETED |
| T | 原子 Provider 回合入口（安全裁决、计数与剩余墙钟预算合并为 `begin_round`） | ✅ COMPLETED |
| U | 终态验收原子归因（headless 的 Accepted/Exhausted 在停止裁决处直接锁定原因） | ✅ COMPLETED |
| V | 原子工具尝试入口（固定/动态预算共用计数入口，终态后不再增加尝试） | ✅ COMPLETED |
| W | 工具尝试统一裁决（计数、固定预算与循环观察合并，策略拒绝也不能绕过循环治理） | ✅ COMPLETED |
| X | 动态工具预算归因入核（扩容保留，最终 Halt 精确归因为工具预算耗尽） | ✅ COMPLETED |
| Y | 桌面完成复核精确归因（验收通过但未确认完成不再误记为 model accepted） | ✅ COMPLETED |
| Z | 回合上限入核（headless 删除外层 `for` 隐式上限，由 `begin_round` 原子裁决） | ✅ COMPLETED |
| AA | 成本预算终态入核（成本超限与首因保持由 executor 原子裁决） | ✅ COMPLETED |
| AB | executor 写入口封闭（终态、自然结束和内部计数不再向 adapter 暴露） | ✅ COMPLETED |
| AC | 唯一最终化入口（固定回合与桌面验收使用类型化模式生成同一快照） | ✅ COMPLETED |
| AD | 运行限制冻结（回合/工具/补救上限创建时注入并进入最终快照） | ✅ COMPLETED |
| AE | 墙钟限制冻结（wall time 纳入运行配置，安全点只接收 elapsed/cancelled） | ✅ COMPLETED |
| AF | 快照版本契约（Durable Run/headless executor snapshot 写入稳定 schema version） | ✅ COMPLETED |

**关键实现细节**：
- headless 保持 fail-closed 语义：流错误不进入中断续写/重放（文档画线）
- KernelLoopGovernor 在每次工具调用前 observe，命中循环时注入纠正提示或直接收尾
- final halt 终止整个 headless round loop，并独立归因为 `tool_loop_exhausted`，不会误报 `max_steps_exceeded`
- KernelRoundRouter 在 stop-candidate 前返回单一 `KernelRoundDecision`，处理空轮/冻结重放/中断续写/截断续写/假调用纠正；中断耗尽注记与后续主控制被显式拆为 `notices + control`，UI/headless 不再各自遍历动作数组推断 continue/break
- ScriptedClient 记录每请求 messages，为后续差分测试提供基础
- 历史 `@文件/@会话` 引用在进入纯策略 assembler 前展开；主动压缩后立即重组本轮请求，并保留尚未发送的一次性注入、图片与续写状态
- 历史工具输出在内核侧执行 1,200 字符上限，避免 adapter 迁移再次取消上下文护栏
- headless 保留流式 `reasoning_content`；UI/headless 的 reasoning-only 截断都会直接要求输出结论，不再漏掉续写指令、重复半截正文或继续消耗推理预算
- headless 外层循环不再维护多个 `stopped_by_*` 布尔值；成本、验收、空轮、工具预算、工具循环和步数耗尽由 `KernelRunState` 锁定唯一主终止原因，`driver_finished.termination_reason` 可直接审计。空轮恰好在最后一步耗尽时不会再误标 `max_steps_exceeded`
- `KernelExecutorState` 成为两个 adapter 共同的跨轮状态所有者，集中装配 `KernelRoundRouter`、`KernelLoopGovernor` 与 `KernelRunState`；Provider/DB/事件/工具执行仍是端口，下一步迁移 IO 外循环
- UI/headless 每次 Provider 请求前都经过 `begin_round`：deadline 优先于用户取消，终止原因进入同一 run state；通过时原子推进回合并返回轮号与剩余墙钟时间，headless 直接用剩余值裁剪单请求 timeout，避免两次读取 elapsed 造成预算漂移
- Provider 回合数与全部模型工具调用尝试（包括策略拒绝前的尝试）由 `KernelExecutorState` 饱和计数；UI/headless 共用 `begin_tool_attempt`，一次完成计数、固定预算裁决与循环观察，headless 传固定 `max_tool_calls`，UI 传动态预算模式并保留扩容策略；权限检查位于统一观察之后，重复的非法尝试同样会被熔断，已终止 executor 不再增加工具尝试
- `KernelExecutorSnapshot` 统一输出 steps/tool attempts/loop breaks/具名 round counters/termination/taxonomy；headless 写入 `driver_finished`，桌面写入 Durable Run 的 `run.executor_snapshot`，不再依赖匿名计数元组或 adapter 私有审计字段
- 最终快照不再允许空终止原因：固定轮数路径用 `finish_and_snapshot` 完成自然耗尽归因，无固定轮数路径用 `terminate_and_snapshot` 提供回退原因；尚未终止且未跑满时失败关闭，避免 trajectory/Durable Run 出现伪 final
- 停止申请的有界补救状态进入 `KernelExecutorState`：UI/headless 都通过 `decide_stop` 推进，同一快照记录 `remediation_rounds`，不再由 UI 局部变量与 headless acceptance gate 各自计数
- `KernelAcceptanceGate` 删除重复的 remediation/max 字段和有状态 `request_stop`，仅保留契约、证据累计与报告生成；停止状态机只有 executor 一个真源
- executor 在产生 `StopEmpty` 或 final tool-loop halt 的同一处自动锁定精确终止原因；两个 adapter 不再补写，桌面快照也不会把这两类终态降级成笼统 governance 归因
- executor 终态成为 Provider 边界的吸收态：`begin_round` 优先返回已经锁定的首因；安全裁决、回合计数与剩余墙钟预算已合并为一次原子操作，UI 对任意终态执行防御性退出，不会意外发起下一轮请求
- 无后置复核门的 headless 使用 `decide_terminal_stop`，Accepted/Exhausted 的决策与精确终止归因不可分离；桌面 UI 继续使用预验收 `decide_stop`，保留 ship 声明审计与完成复核语义
- 桌面动态工具预算通过 executor 的 `decide_dynamic_tool_budget` 裁决，直接复用内部 loop-break 真源；有验证进展时仍可扩容，最终 Halt 原子锁定 `max_tool_calls_exceeded`，不再降级为笼统 governance 归因
- 桌面最终状态由 `finalize_acceptance_snapshot` 统一组合治理耗尽、证据验收与完成确认；证据通过但多轮完成复核仍未确认时归为 `completion_review_exhausted`，任务 UI 与 executor 审计不再互相矛盾
- headless 不再用外层 `for 0..round_limit` 隐式控制步数，改由 `begin_round(Some(limit))` 在下一次 Provider 调用前原子锁定 `max_steps_exceeded`；deadline/cancel 与回合上限的优先级及最终快照均由 executor 单点负责
- headless 成本账本结果统一经过 `observe_cost_budget`：超限时由 executor 锁定 `max_cost_exceeded`，已有更早终态时保持首因；两个生产 adapter 不再直接调用 `kernel_executor.terminate(...)`
- executor 的 `terminate`、`finish`、`termination`、fallback snapshot 与内部计数查询已收为私有；生产 adapter 只能经过回合、工具、预算、验收和最终快照等受控入口推进状态，防止后续重新引入手工终止双写
- `KernelExecutorFinalization` 明确区分 `FixedRounds` 与 `Acceptance`，UI/headless 只通过唯一公共 `finalize(...)` 生成快照；旧的 `finish_and_snapshot`、`finalize_acceptance_snapshot` 与 fallback 快照入口已删除，所有模式保持首因与非空终态约束
- `KernelExecutorLimits` 在生产 executor 创建时一次冻结 round/tool/remediation 上限，`begin_round`、`begin_tool_attempt`、`decide_stop` 与 `finalize` 不再接受可漂移的限制参数；配置随最终快照持久化，固定回合模式缺少创建期上限会失败关闭
- wall time 同样以 `wall_time_ms` 冻结进 `KernelExecutorLimits` 并持久化；`begin_round` 不再接收 adapter 每轮传入的 deadline，只依据冻结契约、elapsed 与取消信号裁决剩余预算
- `KernelExecutorSnapshot` 写入 `schema_version=1`，桌面 Durable Run 与 headless trajectory 共用同一版本化契约；当前快照只表达最终态，不宣称能恢复缺少 governor 完整状态的活跃运行
- Rust lib 共 943 项：935 通过、8 项按环境条件忽略；前端 113 项通过

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

Phase 2/3 与 Phase 4 A—AF 已完成；后续继续收敛单一 IO executor，并保持核心工具不依赖 Docker：

1. 真实 Provider 手动 smoke workflow 与脱敏产物检查已落地
   （`headless-eval-smoke` workflow + `AGENT_EVAL_HARNESS.md` 产物规范）；
2. 流响应读取/停滞治理已在 headless 落地（SSE 行缓冲 + `KernelStreamGovernor` + 失败关闭）；
   桌面 UI 流循环已切换到同一组件：chat.rs 删除本地 `STREAM_SILENT_TIMEOUT`/
   `STREAM_MAX_BYTES`/`REASONING_ONLY_GRACE_SECS` 常量与自维护的 stall deadline/
   reasoning 宽限状态，改用 `KernelStreamGovernor` + `KERNEL_STREAM_*` 常量（含字节
   预算、`sleep_until(deadline)` 硬截止与 200ms tick 兜底判死）；消息历史、tool loop、
   单轮决策也已进入共用内核；
3. 参数级审批、tool metrics 与工具重试语义均已接入 headless runtime：工具执行使用与
   UI 相同的 `TOOL_POLICY` 退避 + `retryable_for` 谓词（契约 retry_safe + 可恢复错误
   白名单，见 `agent/tools/errors.rs`），重试次数写入 trial 私有 `tool_runs.retry_count`，
   每次尝试前重新检查取消与剩余 wall time；血缘式 Recovery Orchestrator（父运行恢复
   计划/核验门）是桌面会话特性，headless 单次 trial 无父运行血缘，按设计不接入；
4. 清理 headless 工具路径上的全局数据库依赖（已核查完成：headless 运行时使用独立
   in-memory 库 + 全量迁移，allowlist 工具全部经 `DbState` 注入，不触 `db::global()`；
   工具分派路径上唯一的全局库调用在 `todo_write`，不在 allowlist。回归测试
   `headless_allowlist_is_exactly_pinned_and_global_db_free` 穷举钉住 allowlist，
   任何新增工具都需显式评审其全局库边界）；
5. 将两个 adapter 的外层 round 编排继续收敛为单一 executor；UI 生命周期、DB IO 与
   headless grader 编排仍作为端口实现保留，不再重复停止、压缩和循环决策。
