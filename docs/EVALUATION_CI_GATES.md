# 评测 CI 基线门禁

本文定义 `EC-15` 的 CI 回退门禁：质量流水线在固定评测基础上保存可跨机器比较的基线，阻止任务完成率、评测覆盖或关键延迟出现显著回退。实现入口为 `src-tauri/src/agent/evals.rs` 的 `EvalBaseline` 与 `compare_with_baseline`，流水线接入见 `.github/workflows/quality.yml`。

## 1. 指标与门禁分工

| 指标 | 来源 | CI 角色 |
|---|---|---|
| 任务完成率 | 固定评测 `agent_harmony_fixed_v3` 的 `score` | 绝对阈值 95%（`reliability_gate`）+ 基线比较（`ci_baseline_gate`） |
| 评测覆盖 | 套件注册场景数 `total_cases` | 基线比较；任何新增场景必须同时实现执行器并 100% 通过 |
| 关键延迟 | 纯内核运行耗时 `duration_ms` | 基线比较 |
| 重复副作用率 | worker/tool worker 崩溃恢复 E2E | 硬性断言为零，不设容差 |
| 恢复率 | worker/tool worker 崩溃恢复 E2E | 硬性断言恢复成功，不设容差 |

固定评测是纯内核运行：不调用模型、不探测 SDK/设备，因此分数和耗时可在 macOS/Windows runner 之间重复。重复副作用率与恢复率继续由 E2E 硬门禁裁决，基线门禁不替代它们。

## 2. 基线生命周期

- 每次 CI 运行结束前，`ci_baseline_gate` 把本次 `EvalRun` 提取为 `EvalBaseline`（schema v1：套件、平台、应用版本、工具注册表摘要与数量、场景数、分数、耗时、时间戳），写入 `src-tauri/target/eval-baseline.json`。
- `actions/cache/restore` 按 `eval-baseline-<os>-` 前缀恢复 main 分支最近一次保存的基线；未命中（首次运行、缓存被驱逐）时不阻断，本次运行直接作为新基线保存。
- 只有 `main` 分支保存新基线；PR 只与 main 的基线比较，避免分支环境污染基线。
- 基线随 `github.sha` 键控，每次 main 提交产生新缓存，旧缓存由 GitHub 自动清理。

## 3. 比较规则与容差

| 指标 | 回退判定 | 严重度 |
|---|---|---|
| `score` | 当前 < 基线 − 0.05（5 个百分点） | fail |
| `total_cases` | 当前 < 基线 × 0.95（覆盖缩水超 5%） | fail |
| `duration_ms` | 当前 > 基线 × 1.5，且基线 ≥ 50 ms（过短基线不比较，避免机器噪声误报） | fail |
| `tool_registry_digest` | 摘要变化（工具增删或描述变更） | warn |
| `producer_version` | 应用版本变化 | warn |
| `suite` | 套件名不一致（评测升级） | warn，且不再比较其余指标 |

任意 `fail` 违规使测试退出非零并阻断合并；`warn` 只出现在日志中。工具集演进是正常开发行为，不允许让 CI 常红；评测套件升级时旧基线自动失效，由下一次 main 运行保存新基线。

## 4. 本地复现

```bash
# 首次运行：无基线时保存基线
EVAL_BASELINE_IN=/tmp/eval-baseline.json EVAL_BASELINE_OUT=/tmp/eval-baseline.json \
  cargo test --manifest-path src-tauri/Cargo.toml --locked agent::evals::tests::ci_baseline_gate -- --exact --nocapture

# 再次运行：与基线比较，回退时测试失败并在日志输出违规清单
EVAL_BASELINE_IN=/tmp/eval-baseline.json EVAL_BASELINE_OUT=/tmp/eval-baseline.json \
  cargo test --manifest-path src-tauri/Cargo.toml --locked agent::evals::tests::ci_baseline_gate -- --exact --nocapture
```

比较逻辑的单元测试 `baseline_comparison_detects_regressions` 覆盖分数回退、覆盖缩水、延迟超限、工具摘要告警、套件切换与过短基线跳过。

## 5. 边界

- 基线是运行证据快照，不是发布验收；真机、SDK 与模型评测证据仍由 [评测运行快照](EVALUATION_RUN_SNAPSHOTS.md) 记录。
- 基线缓存丢失或恢复失败不阻断（首次即基线）；只有比较出显著回退才阻断。
- 人为调低基线或删除缓存不能绕过绝对门禁：`reliability_gate` 仍要求全部注册场景 100% 通过，E2E 仍断言零重复副作用。
