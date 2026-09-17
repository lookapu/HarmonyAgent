# 内置 Provider Headless Agent Driver 设计（可实现版）

> 状态：Phase 0—3 与 Phase 4 A—BA 已落地；headless 生产 adapter 已迁入单一 IO run-loop，桌面 UI 已接续目标不变恢复的 executor、动态预算与已完成证据、完整端口迁移仍待完成
> 更新日期：2026-09-10
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

**Phase 4 实施状态（2026-09-10 更新）**：

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
| AG | 共用 IO 循环壳（`KernelIoRunLoop` 绑定 executor 与单调时钟，UI/headless 不再自行传入 elapsed） | ✅ COMPLETED |
| AH | Provider 边界封口（原始 `with_limits`/`begin_round(elapsed)` 收为私有，生产 adapter 只能经过统一循环壳） | ✅ COMPLETED |
| AI | 异步 IO 端口协议（`KernelIoPort` + `KernelIoRunLoop::run` 统一取消、安全点、轮次驱动与终态退出） | ✅ COMPLETED |
| AJ | 活跃 executor checkpoint（版本化保存 router/governor/计数/首因，恢复时计入停机墙钟） | ✅ COMPLETED |
| AK | checkpoint 参数脱敏（loop 等价键改为稳定 SHA-256 指纹，禁止持久化原始工具参数） | ✅ COMPLETED |
| AL | headless 安全点持久化（下一 Provider 轮前与每个工具结果后写入 executor checkpoint 事件） | ✅ COMPLETED |
| AM | checkpoint 恢复不变量（冻结预算与 router/governor 可达计数校验，篡改/损坏状态失败关闭） | ✅ COMPLETED |
| AN | checkpoint 类型化读取（独立事件不污染消息投影，按 conversation/trace 精确选取最新安全点、复合索引加速并严格恢复） | ✅ COMPLETED |
| AO | 端口可信 checkpoint clock（轮内工具安全点复用内核单调时钟，adapter 不能注入 elapsed） | ✅ COMPLETED |
| AP | Provider 边界 checkpoint hook（统一 run-loop 生成并要求端口持久化，adapter 不再手写时机） | ✅ COMPLETED |
| AQ | headless 生产端口迁移（Provider/路由/验收/工具/事件单轮 IO 进入 `KernelIoPort`，外循环唯一化） | ✅ COMPLETED |
| AR | checkpoint 安全点判型（`tool_result`/`provider_boundary` 来源入事件，工具点在 adapter 状态更新后落盘） | ✅ COMPLETED |
| AS | 桌面活跃 checkpoint（Durable Run 类型化写入/严格恢复，Provider 前受 Worker 租约 fencing） | ✅ COMPLETED |
| AT | 桌面工具安全点（串行结果及只读批次提交后写入 `tool_result` checkpoint，共用严格 helper） | ✅ COMPLETED |
| AU | checkpoint 安全点类型化（UI/headless 共用枚举与 envelope 编解码，恢复返回边界类型，缺失/未知值失败关闭） | ✅ COMPLETED |
| AV | Provider 边界原子化（run-loop 统一执行 checkpoint→轮次裁决，覆盖首轮；写入失败不推进计数或发起 IO） | ✅ COMPLETED |
| AW | 裸轮次入口封口（`begin_next_round` 仅测试构建可见，生产 adapter 只能走持久化原子入口或完整 run-loop） | ✅ COMPLETED |
| AX | 桌面 adapter 高水位（同事务冻结消息/工具审计 rowid、数量与正文占位引用，恢复严格校验漂移） | ✅ COMPLETED |
| AY | 桌面恢复有界物化（子运行创建前严格预检父 checkpoint，只读取高水位内最近 200 条消息/工具审计，专用游标索引并注入无正文摘要） | ✅ COMPLETED |
| AZ | 桌面 executor 血缘接续（仅目标契约不变时恢复停机墙钟/回合/循环/补救状态，checkpoint 同步冻结动态工具额度与扩容次数） | ✅ COMPLETED |
| BA | 恢复证据链接续（只继承恢复计划 `SkipCompleted` 且 id/工具名/成功状态匹配的有界父工具证据，接入工作流与两阶段验收） | ✅ COMPLETED |

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
- `KernelIoRunLoop` 现同时包裹桌面与 headless executor：Provider 请求前只接收 adapter 的取消信号，elapsed 在同一个单调时钟边界采样；headless 工具剩余 wall time 也复用该时钟。它是 IO executor 的统一循环壳，Provider/工具/事件/DB 的异步端口尚待迁入
- 原始 `KernelExecutorState::with_limits` 与 `begin_round(elapsed)` 已收为模块私有；生产 adapter 无法绕开 `KernelIoRunLoop` 注入自算 elapsed 或另建状态所有者
- `KernelIoPort` 把 adapter 限定为“执行一轮 IO 并返回 Continue/Stop”；`KernelIoRunLoop::run` 唯一负责循环、取消采样、Provider 安全点与吸收态退出，并以脚本端口验证 adapter 主动停止和固定回合耗尽两条路径。现有 UI/headless 生产循环尚待迁入该异步入口
- `KernelExecutorCheckpoint` 以独立 schema v1 保存完整 executor 活跃状态；恢复时通过“恢复前累计耗时 + 本进程单调耗时”把停机时长计入 elapsed，不能靠重启或 `Instant` 回溯溢出刷新 wall-time。未知版本或系统时钟倒退会失败关闭；这仍不包含 adapter 的 messages/工具结果等 IO 状态，完整运行恢复需由生产端口组合持久化
- loop governor 的重复调用键已改为长度定界的 SHA-256 指纹；循环语义不变，但 checkpoint 不再复制原始工具参数。headless 在下一 Provider 轮前及每个工具结果后把 checkpoint 同时写入 session event/trajectory，为后续自动恢复保留安全点
- checkpoint 恢复会重新验证冻结的 round/tool/remediation 预算，以及 round router 与 loop governor 计数是否处于运行时可达范围；序列化结构即使能反序列化，也不能携带超限状态绕过治理
- checkpoint 使用独立 `executor_checkpoint` 会话事件，不再作为 system note 派生为空助手消息；`SessionTrajectorySink::restore_latest_executor` 按 conversation + trace + 类型读取最新安全点，专用 `(conversation_id, trace_id, event_type, seq DESC)` 索引避免长会话扫描。最新 payload 损坏、版本未知、时钟倒退或状态不可达都会失败关闭，不会静默降级到旧 checkpoint。当前仍只恢复 executor，不能替代 adapter IO 状态恢复
- `KernelIoClock` 把 run-loop 的单调起点与恢复前累计耗时封装为只读能力，并随 `KernelIoPort::run_round` 借给 adapter；端口可在每个工具结果后调用 `clock.checkpoint(executor)`，但不能改写时钟或向 Provider 治理入口注入自算 elapsed，避免生产迁移为了保留轮内安全点重新打开预算旁路
- `KernelIoRunLoop::run` 在首轮之后、每次尝试进入下一 Provider 边界前生成 checkpoint，并通过 `KernelIoPort::persist_checkpoint` 强制交给 adapter；即使下一步因固定回合上限或已有终态被吸收，也会先留下最后一个已完成回合的状态。端口只实现落库，边界时机和 elapsed 均由内核所有
- headless 的生产 `HeadlessIoPort` 现只实现单轮 Provider、路由、验收、工具与事件 IO；回合循环、Provider 边界安全裁决、停止吸收和边界 checkpoint 全部由 `KernelIoRunLoop::run` 驱动。工具结果后使用端口收到的可信 clock 立即落安全点；集成测试同时断言两类 checkpoint，原有 25 条 headless 差分/治理测试保持通过
- headless checkpoint 事件携带向后兼容的 `safe_point=tool_result|provider_boundary` 元数据；工具安全点调整到 acceptance evidence 与 Provider messages 都更新之后，后续组合 adapter checkpoint 时可据此选择一致恢复边界，不必从相同形状的 executor payload 猜测执行位置
- 桌面 Durable Run 新增 `run.executor_checkpoint` 类型化 API：每轮 Provider IO 前写入版本化 executor 状态，写操作复用 scheduler Worker 租约 fencing，陈旧 Worker 或数据库失败会在外部请求前失败关闭；最新记录恢复复用 schema、停机墙钟和可达状态校验，损坏记录不回退。当前仅接入 executor 状态，不宣称已恢复 UI messages/流缓冲/审批状态
- 桌面串行工具在结果、审计和 `tool_runs` 更新后写入 `tool_result` checkpoint；并行只读工具在每个有界批次按模型顺序提交完成后写入。Provider 与工具路径共用 `persist_desktop_executor_checkpoint`，数据库/租约错误统一失败关闭，不再各自拼接事件 payload
- UI/headless 的 checkpoint 不再接受任意 `safe_point` 字符串：共用 `KernelCheckpointSafePoint` 与扁平 envelope 编解码，恢复 API 同时返回 executor 和已验证边界类型。缺失或未知安全点、损坏 payload、未知 schema 均失败关闭，为后续组合恢复 adapter messages/工具结果提供可判定边界
- `KernelIoRunLoop::begin_persisted_round` 原子包住 Provider 边界 checkpoint 与轮次裁决，UI 不再手写两个可分离步骤；headless 的首轮 Provider 也拥有恢复安全点。持久化失败不会增加 `completed_rounds`，更不会向 adapter 发放外部请求 permit
- 裸 `begin_next_round` 已从生产 API 移除，仅保留为测试探针；非测试构建通过 `cargo check --lib` 验证，UI/headless 无法再绕过 Provider checkpoint 直接取得轮次 permit
- 桌面 checkpoint 现通过 `DesktopAdapterCheckpointCursor` 同事务冻结可见消息与工具审计的 rowid/数量高水位，并绑定正文占位消息引用；payload 不复制正文、工具参数或输出。严格恢复会校验 schema、会话归属、占位角色与集合数量，删除、隐藏或错绑造成的漂移不会静默续跑
- 桌面血缘恢复会在创建子运行前调用 `materialize_latest_desktop_checkpoint`：旧运行没有组合 checkpoint 时兼容原恢复协议；只要最新 checkpoint 存在却损坏或集合漂移就失败关闭。物化查询同时携带 `rowid <= high-water` 与 `LIMIT 200`，通过 migration 081 的消息/工具恢复游标索引从尾部读取并恢复为时间正序；system prompt 与 `recovery.adapter_checkpoint_loaded` 事件仅记录安全点、游标、数量、截断状态及最后记录身份，不复制正文、reasoning、工具参数或输出，也不会把父 executor 的瞬态计数直接灌入新预算
- 目标契约完全未变化的续跑会把父 `KernelIoRunLoop` 移交给子运行，停机墙钟、Provider 回合、工具调用指纹/尝试数、循环熔断首因及验收补救次数不再重置；新增、替换或删除目标要求时只使用已验证数据边界并新建 executor，`recovery.adapter_checkpoint_loaded` 明确审计是否接续及重置原因
- `desktop_adapter_control` schema v1 与 executor/cursor 在同一 checkpoint 事件中冻结 `effective_tool_rounds` 和 `budget_extensions`；工具额度判定改用 executor 的跨血缘累计 attempt，而非当前进程内 `tool_runs.len()`。Phase AX/AY 的旧 checkpoint 缺少 control 时允许读取，但保守禁用再次扩容，避免恢复反复刷新动态预算
- `RecoveryDecision` 新增向后兼容的 `external_id`，父工具证据只有在目标契约未变化、恢复动作是 `SkipCompleted`，并且 checkpoint 窗口内的 `tool_runs.id`、工具名和 `status=ok` 同时匹配时才可继承。继承证据按父 rowid 顺序置于本轮证据之前，进入 workflow snapshot、申请完成时的 remediation gate 与最终 acceptance；窗口截断或旧计划缺 ID 时宁可要求重新验证
- 继承证据不写入当前 `tool_runs`：不会重复追加工具消息、重复持久化、污染本轮进度/账本或触发同一个外部动作；但 ship 声明审计与完成复核会把它视作执行型任务证据，避免恢复后无新工具时被误当成纯问答自动完成
- Rust lib 共 989 项：980 通过、9 项按环境条件忽略；前端 113 项通过

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

Phase 2/3 与 Phase 4 A—BA 已完成；后续继续把桌面 UI adapter 迁入单一 IO executor，并保持核心工具不依赖 Docker：

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

## 17. 桌面 IO port 迁移：可执行分步方案（2026-09-15 调研，未开始改）

第 16 节第 5 条是当前唯一剩下的深度重构。本轮做了定位调研，把「为什么不能一次改完」和「按什么顺序改」写清楚，避免下一轮从零推导。

**现状与体量（实测）**

| 事实 | 数值/位置 |
| --- | --- |
| `KernelIoPort` 接口 | 3 个方法：`cancelled()`、`persist_checkpoint(checkpoint)`、`run_round(executor, clock, round, remaining)`（`agent/kernel_executor.rs:184`） |
| 桌面主循环体 | `commands/chat.rs` 的 `'outer: loop`，**约 2,107 行**（4425 → 6531） |
| 桌面当前接法 | 循环内调用 `kernel_executor.begin_persisted_round(is_cancelled(..), |checkpoint| persist_desktop_executor_checkpoint(..))`（4476 行），round 体是**内联在循环里**的代码，不是一个函数 |
| headless 对照 | `headless_driver.rs` 的 `HeadlessIoPort` 已实现该 trait，外层用 `KernelIoRunLoop::run(port)` |
| 事件发射点 | `chat.rs` 内 52 处 `emit`/`emit_log`，大量位于 round 体内 |

**为什么不能一次改完**：`run_round` 要求把「一轮的全部工作」做成一个可调用的函数，而它现在依赖几十个循环内可变局部变量（预算、账本、`workflow_stage`、`exhausted`、`completion_reviews`、`merged_instructions`、`placeholder_msg_id`、`stats`…）。把这些搬进结构体再搬回来，等价于重写主循环的状态机；在没有 GUI/headless 端到端冒烟的前提下，单测与两组 crash E2E 只能覆盖「检查点/恢复/取消」这些协议面，覆盖不了「一轮里 52 个事件按顺序发对、预算与账本推进正确」这类行为。一次改完的风险是静默的行为回归。

**分步方案（每步都必须独立可验证、可提交）**

1. **冻结现状基线**：为桌面一键路径补一组「行为快照」测试——用注入的假 Provider（已有 `LlmProvider` 抽象）跑一条固定剧本，断言：事件序列（类型 + 顺序）、账本推进、预算计数、终态。这一步只加测试，不改行为；它是后续每一步的安全网。
2. **提取 round 体**（最大的一步）：把 `'outer: loop` 内的 round 体搬进 `async fn desktop_round(...)`，用显式参数结构体传入、用返回结构体传回；`break`/`continue` 换成枚举返回值。**纯搬运、零行为改动**，靠第 1 步的快照测试 + 全量回归把关；此时循环仍用 `begin_persisted_round`，不换执行器。
3. **实现 `DesktopIoPort`**：`cancelled()` 委托现有 `is_cancelled`；`persist_checkpoint()` 委托 `persist_desktop_executor_checkpoint`；`run_round()` 调用第 2 步的 `desktop_round`。此时新类型尚未接入路径（若不能立即接入，就先与第 4 步合并提交，避免留下无人调用的代码）。
4. **切换执行器**：用 `KernelIoRunLoop::run(port)` 替换 `begin_persisted_round` + 内联体，删除旧路径；确认检查点/恢复、取消、超时三条链路的既有测试与两组 crash E2E 全绿。
5. **清理与对齐**：核对桌面与 headless 在「停止、上下文压缩、循环决策」上不再各写一份；更新 [当前状态单页](./CURRENT_STATUS.md) 的对应行与盘点日志。

**验证要求（缺一不可）**：每步跑后端库全量 + `worker_crash_e2e` + `tool_worker_crash_e2e`；第 2、4 步额外要求第 1 步的事件序列快照不变；第 4 步之后需要在真实桌面里手动跑一条「多轮工具任务 + 中途停止 + 断点续跑」，本轮无 GUI 验收环境，因此**第 4 步不应在无桌面验收窗口的批次里执行**。

**当前结论**：第 1、2 步（加安全网 + 纯搬运）风险可控，可作为下一批目标；第 3、4 步需要同时具备桌面验收条件。本轮只做调研与方案，未改动 `chat.rs`。

## 18. 桌面 IO port 迁移：第 1 步的前置条件已查明（2026-09-15 调研，仍未改 `chat.rs`）

第 17 节把「补行为快照测试」列为第 1 步，本轮动手前先验证它能不能做，结论是**不能直接做**，但解法已经找到。

**为什么做不了（实测）**

- `stream_chat_inner`（`commands/chat.rs:2730`）直接接收 `&AppHandle` 与四个 `tauri::State`（`DbState`/`ChatLock`/`ChatCancel`/`ToolApprovalState`/`PlanApprovalState`），`stream_once`（7417）同样吃 `&AppHandle` + `&State<DbState>`；仓库里没有任何 Tauri 测试替身，`tauri` 依赖也没启用 `test` feature（`Cargo.toml:36` 只有 `tray-icon`/`protocol-asset`）。因此「用假 Provider 跑固定剧本、断言事件序列」在现状下无处落脚。
- 主循环体内（4425→6531，2,107 行）的接线密度：**32 处 `.emit(`**、**22 处 Tauri State / DB 锁**、13 处 `kernel_executor.*`、12 处持久化调用（`persist_turn`/`append_event`/`record_run_event`）、18 行上下文压缩相关；该函数主循环之外还有 6 处 emit 与 31 处 State/DB 触达。这就是「纯搬运」需要一次性处理的可变捕获面。

**解法（两个前置件，都不需要先做迁移）**

1. **确定性 LLM**：仓库已有 `services/llm_replay` 录制/重放接缝（`chat.rs:10803` 起），`ReplayMode::Replay(dir)` 命中时直接返回录制文本、不发真实请求，且已被评测链路使用。用它给一键路径提供确定性回复，**不必改生产代码**（录制一次 fixture 即可）。
2. **Tauri 测试替身**：给 `tauri` 加 `test` feature（仅测试依赖）并用 `tauri::test::mock_builder` 构造带托管状态的 app（`DbState` 用内存库跑全量迁移、`ChatCancel`/`ToolApprovalState`/`PlanApprovalState`/`TaskRegistry` 直接 `manage`），即可在测试里调用 `stream_chat_inner`。

**更正后的顺序**（0 是新增的前置；原 1—5 顺延）

0. **补测试替身**：`tauri` test feature + mock app 装配 + 一份 replay fixture；先拿一条最小剧本（单轮、无工具）跑通并断言终态。
1. **行为快照**：固定剧本（多轮 + 工具调用 + 中途停止 + 断点续跑）断言事件序列（类型与顺序）、账本推进、预算计数、终态。
2. **纯搬运** round 体到 `desktop_round(...)`（零行为改动，第 1 步守护）。
3. **实现 `DesktopIoPort`**（`cancelled`/`persist_checkpoint`/`run_round` 三方法，见 `kernel_executor.rs:184`）。
4. **切换执行器**：`KernelIoRunLoop::run(port)` 替换 `begin_persisted_round` + 内联体，删旧路径。
5. **清理与对齐**：核对桌面与 headless 不再各写一份停止/压缩/循环决策，更新状态单页与盘点日志。

**当前结论**：第 0 步是迁移的真正前置，且比迁移本身小、可独立验证；在没有它之前**不要开始第 2 步的 2,107 行搬运**——否则等于在没有安全网的情况下重写应用主循环。本轮只做调研与计划更正，未改动 `chat.rs`。

## 19. 第二次更正：Tauri 测试替身这条路走不通，端口抽取本身就是前置（2026-09-15，实测）

按第 18 节动手做「第 0 步：Tauri 测试替身」，实测后**否掉了自己上一节的方案**。

**做了什么、撞到什么**

给 `tauri` 加 `test` feature 后，`MockRuntime` 与生产函数的 `AppHandle`（默认 `Wry`）**是不同的类型**，所以必须先把桌面路径对 runtime 泛型化。照着改：chat.rs 里 18 处 `app: &AppHandle` 中 17 处在普通函数（1 处在 `#[tauri::command] delete_conversation`，命令不能泛型），加 `<R: tauri::Runtime>` 后编译立刻报出 5 类连锁：

| 连锁点 | 为什么不能只改 chat.rs |
| --- | --- |
| `generate_conversation_title(&app, …)` | 同文件另一个吃 `&AppHandle` 的函数（参数名不同，第一轮没匹配到） |
| `services::harmony_docs::docs_root(&app)` | service 层也有吃 `&AppHandle` 的函数 |
| `exec_ctx::ToolCtx::new(app.clone(), …)` | **`ToolCtx` 这个类型里存着 `AppHandle`**——`R` 会扩散到所有工具、Broker、exec_ctx |
| `StreamEventBatcher::new(app, …)` | 同类：结构体嵌 `AppHandle` |
| … | 继续修会一路扩散到整个 agent 层（数百处签名与结构体） |

结论：**Tauri mock 替身的代价 ≈ 全仓 AppHandle 泛型化**，这不是"前置小步"，而是一次比 IO port 迁移本身更大的重构。已全部回退（`git checkout chat.rs` + 移除 dev-dependency），工作树编译干净、1,103 项测试全绿。

**更正后的结论（第 18 节第 0 步作废）**

1. 桌面路径在现状下**没有便宜的自动化驱动方式**；也不要为了测试去做全仓 AppHandle 泛型化。
2. 正确的顺序是**先抽取、后加网**：端口抽取（把 round 体搬成不依赖 Tauri 的函数/端口）本身就是让逻辑可测的前置——抽出来之后，用**假端口**写行为快照，不需要 Tauri 替身。
3. 因此抽取那一步（2,107 行、32 处 emit、22 处 State/DB）在完成前**没有自动化安全网**，必须按三条纪律做：① 纯搬运、零逻辑改动，每步一个提交；② 靠编译器 + 全量回归 + 两组 crash E2E；③ **切换到 `run(port)` 之前必须有真实桌面手动验收**（多轮工具任务 + 中途停止 + 断点续跑），本机无 GUI 条件，因此第 4 步只能在有桌面环境的批次里做。
4. 抽取的推荐切法（降低单次搬运量）：按"每轮三段"拆——轮前（阶段快照/预算/许可）、轮中（Provider 请求 + 工具执行）、轮后（持久化/账本/提示注入）；每段先抽成函数、仍接受 Tauri 类型，再在端口落地时统一改签名。这样每步的 diff 可读、可回退。

本轮只做调研与回退，未留下任何代码改动。
