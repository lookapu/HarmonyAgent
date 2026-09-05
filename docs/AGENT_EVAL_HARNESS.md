# Headless Agent Eval Harness 设计

> 状态：Phase 0 接口草案  
> 更新日期：2026-09-03

## 1. 目的

现有 `agent_harmony_fixed_v3` 是确定性内核评测：它不调用真实模型，也不让 Agent 在隔离仓库中完成开放任务。该套件继续承担快速回归门禁；本文定义另一条真实 Agent 产品评测路径。

首个目标是让一个任务从 JSON 输入，经真实 Agent loop、工具与执行环境，产出可由外部 grader 判断的 patch、trajectory 和 report。评测入口必须与 Tauri UI 解耦，但复用同一执行内核。

## 2. 命令行契约

计划入口：

```bash
harmony-agent eval run \
  --task eval/tasks/example.json \
  --workspace /absolute/repo \
  --model-config eval/model.json \
  --sandbox oci \
  --output eval-runs/example
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

`manifest.json`、`trajectory.jsonl`、`report.json` 已分别落地为 `agent::eval_report` 的 `EvalManifest`/`EvalReport` 与 `agent::eval_trajectory` 的 `TrajectoryWriter`（统一事件信封 + JSONL 落盘 + 边写边算 SHA-256）；事件源（AgentEventSink）待抽取后接入 `TrajectoryWriter`。

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

不能把 `simulate_scenario` 扩展成假的真实模型评测；确定性 fixture 与真实 trial 必须使用不同 suite 类型和报告字段。

## 10. 首个实现切片

- [x] 抽取 `AgentEventSink`，让 Tauri 和 JSONL writer 共用事件源（改用拉取式桥接：`eval_trajectory::session_events_to_trajectory` 直接回放 `session_events` 到 trajectory.jsonl，复用真实事件源，无需再引入 push sink trait）；
- [ ] 增加只接受本地已准备 workspace 的 `eval run`；
- [ ] 只支持一个 Provider、`network=none` 和 command grader（command grader 已落地为 `agent::eval_grader`：argv 直接执行、退出码判定、超时兜底、拒绝 shell 解释器与绝对路径；Provider 接线与 `network=none` 随 runner）；
- [ ] 输出完整 manifest/trajectory/patch/report（manifest/report/trajectory 数据契约与 patch 采集/应用 `agent::eval_patch`（`git diff base` / `git apply`）已落地；由 `eval run` runner 组装成四件套待做）；
- [ ] 用一个 5 分钟内可完成的小仓任务作为 CI 手动 workflow artifact；
- [x] 未交付真实沙箱前，runner 必须拒绝不可信 task，而不是回退宿主执行（已落地为 `agent::eval_task`：task schema v1 解析 + 安全校验，拒绝宿主命令/绝对路径/`..`/命令替换/联网/不安全 artifact，并附单元测试）。

已完成部分见 `src-tauri/src/agent/eval_task.rs`、`eval_report.rs`、`eval_trajectory.rs`、`eval_grader.rs`、`eval_patch.rs`；其余切片（`eval run` runner、Provider 接线、CI artifact）待实现。

相关文档：[固定评测集](FIXED_EVALUATION_SUITE.md)、[评测运行快照](EVALUATION_RUN_SNAPSHOTS.md)、[安全边界](SECURITY_BOUNDARY.md)、[演进路线](AGENT_EVOLUTION_ROADMAP_2026.md)。
