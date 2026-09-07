# Headless Agent Eval Harness 设计

> 状态：Phase 0 runner 已形成可复现评测包，真实 headless 驱动与 CLI 待接入
> 更新日期：2026-09-07

## 1. 目的

现有 `agent_harmony_fixed_v3` 是确定性内核评测：它不调用真实模型，也不让 Agent 在隔离仓库中完成开放任务。该套件继续承担快速回归门禁；本文定义另一条真实 Agent 产品评测路径。

首个目标是让一个任务从 JSON 输入，经真实 Agent loop、工具与执行环境，产出可由外部 grader 判断的 patch、trajectory 和 report。评测入口必须与 Tauri UI 解耦，但复用同一执行内核。

## 2. 命令行契约

当前入口（CLI 通过显式 feature 构建，不进入 Tauri 桌面包）：

```bash
cargo run --manifest-path src-tauri/Cargo.toml --features eval-cli --bin harmony-agent -- eval run \
  --task eval/tasks/example.json \
  --workspace /absolute/repo \
  --run-config eval/run-config.json \
  --driver /absolute/path/to/trusted-agent-adapter \
  --driver-arg adapter-specific-value \
  --output eval-runs/example
```

`--output` 必须是尚不存在的新目录，防止覆盖旧 trial。task 与 run config 限制为 1 MiB 普通 UTF-8 文件；workspace、driver 在执行前规范化为真实路径，driver 必须是绝对可执行文件。`--driver-arg` 可重复。

`run-config.json` 不保存 API key，只保存可复现指纹：

```json
{
  "run_id": "example-001",
  "suite_version": "1",
  "grader_version": "command-v1",
  "harness": { "commit": "full-git-sha", "app_version": "2.1.1", "platform": "macos-arm64" },
  "model": { "provider": "provider-name", "model_id": "exact-model-id", "protocol": "openai", "reasoning_effort": "high" },
  "prompt": { "profile_version": "v1", "digest": "sha256:<64-hex>" },
  "tool_registry": { "version": "v1", "digest": "sha256:<64-hex>" },
  "sandbox": { "backend": "process-adapter", "capabilities": "workspace-write", "image_digest": null, "network_policy": "none" }
}
```

退出码：

- `0`：trial 正常执行且 grader 通过；
- `1`：trial 正常执行但 grader 未通过；
- `2`：输入、环境、模型、沙箱或 harness 错误，不能计为有效 trial；
- `130`：用户或调度器取消。

## 3. Task schema v1

```json
{
  "schema_version": 1,
  "task_id": "suite__case-id",
  "suite": "harmonybench-smoke-v0",
  "problem_statement": "修复给定问题并验证结果。",
  "repo": {
    "url": "https://example.invalid/repo.git",
    "base_commit": "full-commit-sha",
    "subdir": null
  },
  "limits": {
    "wall_time_seconds": 1800,
    "max_steps": 200,
    "max_cost_cny": 20.0,
    "network": "none"
  },
  "grader": {
    "kind": "command",
    "command": ["npm", "test"],
    "timeout_seconds": 600
  },
  "artifacts": ["test-results/**"]
}
```

Task 文件不能携带宿主命令或凭据。外部数据集 adapter 必须把 grader 映射到受信任的本地注册表或固定镜像，而不是直接执行下载数据中的任意字符串。

## 4. 输出目录

```text
eval-runs/<run-id>/
├── manifest.json
├── trajectory.jsonl
├── model.patch
├── report.json
├── grader/
│   ├── stdout.log
│   └── stderr.log
└── artifacts/
```

`manifest.json` 固定运行条件；`trajectory.jsonl` 保存事件流；`report.json` 只保存 grader 结论和派生指标。三者不得相互替代。

`manifest.json`、`trajectory.jsonl`、`report.json` 已分别落地为 `agent::eval_report` 的 `EvalManifest`/`EvalReport` 与 `agent::eval_trajectory` 的 `TrajectoryWriter`（统一事件信封 + JSONL 落盘 + 边写边算 SHA-256）。`run_trial` 已把驱动返回的真实计量与事件写入评测包；后续 headless 驱动必须复用 `session_events` 事件源，不能从最终文本反推轨迹。

runner 要求调用方显式提供 harness/model/prompt/tool/sandbox 指纹，拒绝用空值生成看似可复现的报告；manifest 还记录规范化 task JSON 的 SHA-256，防止相同 task id 下题目内容被静默替换。`repo.subdir` 会同时约束 Agent 工作目录、grader 工作目录与声明产物根，而补丁仍从完整仓库根采集。

## 5. Report schema v1 必填字段

- harness commit、应用版本、平台；
- model provider、精确 model id、protocol、reasoning effort；
- prompt profile version/digest、tool registry version/digest；
- task suite/version/id、repo base commit；
- sandbox backend、capabilities、image digest、network policy；
- started/finished/duration、token、cost、steps、tool calls、retries；
- patch digest、trajectory digest、grader kind/version；
- `resolved | unresolved | harness_error | cancelled`；
- FAIL_TO_PASS/PASS_TO_PASS 或 Harmony outcome assertions；
- failure taxonomy 和安全策略违反计数。

以上必填字段已落地为 `agent::eval_report`（`EvalReport` 类型 + JSON 序列化 + 字段完整性单测）；runner 完成后直接采集各字段即可，无需再定义 schema。

## 6. 执行状态机

```text
validate input
  -> prepare immutable base + task worktree
  -> prepare sandbox
  -> start durable Agent run
  -> stream trajectory
  -> collect patch and declared artifacts
  -> destroy Agent sandbox
  -> grade in independent clean sandbox
  -> write/digest report
```

Agent 运行容器与 grader 容器必须分离。Agent 不得看到隐藏测试、gold patch 或 grader 输出；grader 从原始 base commit 应用 `model.patch` 后执行。

## 7. 可比性规则

- A/B 只改变一个注册变量：model、prompt、tool policy、retrieval 或 orchestration；
- 固定数据集 revision、镜像 digest、token/成本/时间预算；
- 对随机模型运行多次 trial，报告逐题结果和置信区间；
- 无效环境运行不计 unresolved，但必须单列 harness error rate；
- 不只报告平均分，同时发布成本/成功任务、wall time 和失败分类；
- 公开结果附 predictions、patch、trajectory、grader logs 和 reproduction command。

## 8. Adapter 顺序

1. 本地单任务 adapter，打通真实 Agent loop；
2. SWE-bench Verified 25 题 smoke + 官方 Docker grader；
3. HarmonyBench 20 题 smoke；
4. SWE-Explore 文件/行定位 adapter；
5. Verified 100 与 HarmonyBench 50 周回归；
6. Verified 500、SWE-bench Pro/Live 里程碑运行。

## 9. 与现有代码的复用边界

必须复用：`execution_loop`、`runtime`、`coordinator`、`recovery`、`acceptance`、工具协议、tool metrics 和事件模型。

可以替换：UI event sink、Provider 配置来源、workspace provisioner、sandbox backend、grader adapter 和 artifact sink。

真实驱动接入已定义 `ProcessAgentDriver` 适配协议：受信任的本地 adapter 以 Agent 工作树为当前目录，从 stdin 接收完整 `EvalTask` JSON，并在 stdout 返回 `AgentDriverOutcome` JSON；诊断写 stderr。runner 负责 wall-time 终止、退出码判定和失败/取消评测包收敛。该协议允许先接外部 Provider adapter，之后再把同一接口替换为内置 headless loop。

不能把 `simulate_scenario` 扩展成假的真实模型评测；确定性 fixture 与真实 trial 必须使用不同 suite 类型和报告字段。

## 10. 首个实现切片

- [x] 抽取 `AgentEventSink`，让 Tauri 和 JSONL writer 共用事件源（改用拉取式桥接：`eval_trajectory::session_events_to_trajectory` 直接回放 `session_events` 到 trajectory.jsonl，复用真实事件源，无需再引入 push sink trait）；
- [x] 增加只接受本地已准备 workspace 的 `eval run`（`harmony-agent eval run` CLI 与 `ProcessAgentDriver` 已接通；内置 Provider headless loop 仍待从 `commands/chat.rs` 抽取）；
- [ ] 只支持一个 Provider、`network=none` 和 command grader（command grader 已落地为 `agent::eval_grader`：argv 直接执行、退出码判定、超时兜底、拒绝 shell 解释器与绝对路径；Provider 接线与 `network=none` 随 runner）；
- [x] 输出完整 manifest/trajectory/patch/report（`run_trial` 对 resolved/unresolved/harness_error/cancelled 均生成四件套和 grader stdout/stderr；patch/trajectory 摘要与磁盘内容交叉验证）；
- [ ] 用一个 5 分钟内可完成的小仓任务作为 CI 手动 workflow artifact；
- [x] 未交付真实沙箱前，runner 必须拒绝不可信 task，而不是回退宿主执行（已落地为 `agent::eval_task`：task schema v1 解析 + 安全校验，拒绝宿主命令/绝对路径/`..`/命令替换/联网/不安全 artifact，并附单元测试）。

已完成部分见 `src-tauri/src/agent/eval_task.rs`、`eval_report.rs`、`eval_trajectory.rs`、`eval_grader.rs`、`eval_patch.rs`、`eval_workspace.rs`、`eval_runner.rs`；主路径剩余工作是真实 headless `AgentDriver` 实现（从 `commands/chat.rs` 抽取）、CLI 入口与 CI artifact。

相关文档：[固定评测集](FIXED_EVALUATION_SUITE.md)、[评测运行快照](EVALUATION_RUN_SNAPSHOTS.md)、[安全边界](SECURITY_BOUNDARY.md)、[演进路线](AGENT_EVOLUTION_ROADMAP_2026.md)。
