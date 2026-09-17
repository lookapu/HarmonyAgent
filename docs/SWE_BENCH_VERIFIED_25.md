# SWE-bench Verified 25 固定 Smoke 子集

> 状态：v1 固定清单与校验器已实现；官方 gold 25/25 容器自检保留为可选外部 CI 验收。
> 数据集：`princeton-nlp/SWE-bench_Verified`，`test` split（500 题）。
> 数据集 revision：`c104f840cc67f8b6eec6f759ebc8b2693d585d4a`

## 目的

这 25 题用于验证 harness、Agent、patch、grader 和报告链路是否可复现，不用于追榜。HarmonyAgent 本体不依赖 Docker；需要对齐官方 SWE-bench 容器结果时，由独立 CI 适配器执行。
实例选择一旦发布就只新增版本，不原地替换，以免历史趋势失去可比性。

## 清单契约

代码真源为 `agent::eval_suite::FixedEvalSuite`，manifest 必须包含：

- `schema_version=1`；
- 稳定 `suite_id`；
- 精确 dataset 与 split；
- `expected_instances=25`；
- 25 个唯一、格式合法的官方 `instance_id`。

校验器拒绝数量漂移、重复 ID、非法 ID 和未知 schema。运行器接入后必须先校验 manifest，
再准备仓库或容器，避免错误清单消耗评测资源。

## 选择规则

首版清单应从官方 Verified 500 题元数据确定性生成：

1. 按 repository 分层，避免单一仓库占据多数；
2. 覆盖测试、解析、数据模型、Web 框架等不同代码形态；
3. 排除官方 harness 无法构建或 gold patch 不能通过的实例；
4. 固定实例 ID、dataset revision 和选择脚本摘要；
5. 用官方 gold patch 先完成 25/25 grader 自检；
6. 保存容器镜像 digest、运行日志和 `results.json`。

## 完成门槛

- 仓库内存在版本化的 25-ID JSON manifest；
- `validate_fixed_suite` 通过；
- 官方 gold patch 在固定 harness/container revision 下 25/25 通过；
- builtin driver 至少完成一次真实 trial 并生成 manifest/trajectory/patch/report；
- CI 提供手动 workflow，默认不消耗模型额度。

固定清单位于 `evals/suites/swe-bench-verified-smoke-25-v1.json`，覆盖 12 个上游仓库，
并包含快速、中等和 1–4 小时三档难度。实例 ID 与 dataset revision 均来自官方
Hugging Face 数据集 API；后续只允许新增 `v2`，不得原地替换 v1。
